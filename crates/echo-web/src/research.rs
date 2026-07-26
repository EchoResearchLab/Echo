//! Echo Research 研究页（Leptos/WASM）。
//!
//! 布局：左侧会话历史 + 右侧对话流 + 常驻编辑器。信息优先级是 答案 > 证据摘要 > 证据明细，
//! 所以流式期间不把 meta 骨架铺在答案上方——骨架只占一行摘要条，落地后原地变成可展开的
//! 证据面板，答案的位置从头到尾不跳。
//!
//! 作答走类型化 SSE（`/api/ask/stream`）：stage 严格按 `route.plan` 的步骤名与序号推进，
//! meta 提供路由/估值骨架，delta 是打字机增量，guard 是事实护栏，final 是落库结果；
//! `error` 或连接异常归一到失败态。
//!
//! 响应式粒度：每个 turn 自带 `RwSignal`，delta 到达只重渲染那一张卡，不重建整条对话的
//! DOM——否则长会话里每个 token 都要重挂一遍全部消息，动画也会被反复重启。

use crate::api;
use crate::dialog::confirm_destructive;
use crate::format;
use crate::icons::{EchoArt, Icon};
use echo_contracts::{
    AnswerSource, AskRequest, AskResponse, CitationGuardView, CompanyResolveResponse,
    CompanySearchItem, CompanySearchResponse, CompareLegView, CompareResponse, Decimal,
    EarningsCalendarView, EvidenceView, GuardView, MutationResponse, ReportGenerateResponse,
    ReportMode, ResearchSessionDetail, ResearchSessionResponse, ResearchSessionsResponse,
    ResearchStreamEvent, ResearchStreamStage, ResearchStreamStageName, RouteView, ValuationView,
};
use leptos::*;

/// 流式研究的整体超时窗口（毫秒）。一次性定时器，不随事件重置：多阶段研究本就该在这个
/// 窗口内跑完，卡死比慢更值得暴露。
const STREAM_TIMEOUT_MS: i32 = 120_000;
/// 深度报告是单请求非流式调用，没有中途事件可判活，给一个更宽但仍然有限的窗口。
const REPORT_TIMEOUT_MS: i32 = 240_000;

/// 一轮的终态或进行态。`Streaming` 里的字段随 SSE 事件逐步填充。
#[derive(Clone)]
enum TurnStatus {
    Streaming {
        stage: Option<ResearchStreamStage>,
        meta_route: Option<RouteView>,
        meta_valuation: Option<ValuationView>,
        meta_completeness: Option<u8>,
        meta_sources: Vec<String>,
        meta_earnings: Option<EarningsCalendarView>,
        delta_text: String,
        guard: Option<GuardView>,
    },
    Done(AskResponse),
    /// 对话内双主体对比完成——两腿独立取数/独立护栏；对比结果暂不落库。
    CompareDone(Box<CompareResponse>),
    /// 从历史会话加载的一轮——字段比 [`AskResponse`] 少（当时未持久化路由/完备度/护栏明细），
    /// 缺的就是缺的，不拿假数据补全。
    Archived(Box<ArchivedTurn>),
    Failed(String),
    Cancelled,
    /// 深度报告——非流式单请求（`POST /api/report/generate`），进行中/完成两态。
    ReportPending,
    ReportDone(Box<ReportGenerateResponse>),
}

impl TurnStatus {
    fn streaming_default() -> Self {
        Self::Streaming {
            stage: None,
            meta_route: None,
            meta_valuation: None,
            meta_completeness: None,
            meta_sources: Vec::new(),
            meta_earnings: None,
            delta_text: String::new(),
            guard: None,
        }
    }

    fn is_streaming(&self) -> bool {
        matches!(self, Self::Streaming { .. })
    }

    /// 占用提交通道——流式研究进行中，或深度报告正在生成。
    fn is_busy(&self) -> bool {
        matches!(self, Self::Streaming { .. } | Self::ReportPending)
    }
}

/// 会话历史里的一轮问答（服务端 `thread_json` 的元素）。
#[derive(Clone, serde::Deserialize)]
struct HistoryTurn {
    question: String,
    answer: String,
}

/// 历史会话还原出的一轮。
#[derive(Clone)]
struct ArchivedTurn {
    answer: Option<String>,
    /// 只在会话第一轮显示，标明这段记录的归档时间。
    created_at: Option<String>,
    /// 只有最后一轮挂证据面板——估值与数据源是会话级快照，不是每轮各自的口径，
    /// 挂在每一轮上会让人以为早期轮次也核过这些数字。
    evidence: Option<ArchivedEvidence>,
}

/// 会话级的证据快照（落库时实际存下来的那部分）。
#[derive(Clone)]
struct ArchivedEvidence {
    valuation: Option<ValuationView>,
    sources: Vec<String>,
}

/// 一条对话轮——用户问题 + 助手作答的当前状态。
///
/// 状态放在自己的信号里：流式增量只惊动这一轮，用户气泡不会被反复重建。
#[derive(Clone, Copy)]
struct Turn {
    /// 提交时分配的唯一 id——SSE 回调按 id 归位，不按"最后一条"猜测。
    id: u64,
    question: StoredValue<String>,
    ticker: RwSignal<String>,
    status: RwSignal<TurnStatus>,
    /// 仍在流式进行时持有取消句柄；终态后清空，避免悬空取消一个已结束的请求。
    handle: StoredValue<Option<api::StreamHandle>>,
    /// 深度报告 turn 失败后重试要走报告通道，不能落回默认的 SSE 问答通道。
    is_report: bool,
}

impl Turn {
    fn new(id: u64, question: String, ticker: String, status: TurnStatus, is_report: bool) -> Self {
        Self {
            id,
            question: store_value(question),
            ticker: create_rw_signal(ticker),
            status: create_rw_signal(status),
            handle: store_value(None),
            is_report,
        }
    }

    /// 主动取消：先中止底层请求，再落到终态。已是终态就什么都不做。
    fn cancel(&self) {
        if !self.status.get_untracked().is_busy() {
            return;
        }
        if let Some(handle) = self.handle.get_value() {
            handle.cancel();
        }
        self.handle.set_value(None);
        self.status.set(TurnStatus::Cancelled);
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────

fn intent_label(s: &str) -> &str {
    match s {
        "valuation" => "估值判断",
        "financial_quality" => "利润质量",
        "moat" => "护城河",
        "falsification" => "证伪条件",
        "comparison" => "对比研究",
        "momentum" => "动量与预期",
        "risk" => "风险与赔率",
        "thesis" => "多空逻辑",
        _ => "综合研究",
    }
}

fn stage_label(stage: Option<&ResearchStreamStage>) -> String {
    let label = match stage.map(|stage| stage.name) {
        None => "正在规划研究路径…",
        Some(ResearchStreamStageName::Routing) => "正在判断研究意图…",
        Some(ResearchStreamStageName::Resolving) => "正在确认研究主体…",
        Some(ResearchStreamStageName::MarketFinancials) => "正在核对行情与财报…",
        Some(ResearchStreamStageName::Evidence) => "正在检索网页证据…",
        Some(ResearchStreamStageName::Valuation) => "正在构建估值框架…",
        Some(ResearchStreamStageName::Generating) => "正在综合证据并作答…",
        Some(ResearchStreamStageName::FactCheck) => "正在核对事实与引用…",
        Some(ResearchStreamStageName::Assembling) => "正在组装事实…",
        Some(ResearchStreamStageName::Verifying) => "正在核对数字护栏…",
        Some(ResearchStreamStageName::Persisting) => "正在落库…",
    };
    match stage {
        Some(stage) if stage.index > 0 && stage.total > 0 => {
            format!("第 {}/{} 步 · {label}", stage.index, stage.total)
        }
        _ => label.to_string(),
    }
}

pub(crate) fn decimal_text(value: Option<Decimal>) -> String {
    value
        .map(|decimal| decimal.normalize().to_string())
        .unwrap_or_else(|| "—".to_string())
}

/// 公司候选的展示标签——优先中文名，缺了退中文名/英文名/代码本身，不留空。
fn company_display(name_zh: &str, name_en: Option<&str>, ticker: &str) -> String {
    let name = non_empty(name_zh)
        .or_else(|| name_en.and_then(non_empty))
        .unwrap_or_else(|| ticker.to_string());
    format!("{name} · {ticker}")
}

fn non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// 把一次研究请求接到类型化 SSE 流上：事件回来后写进这一轮自己的信号，
/// 迟到事件（turn 已是别的终态）一律忽略。带 `session_id` 时后端把这轮追加到
/// 同一研究会话（历史只帮代词/实体承接，不注入旧数字）；`Final` 落库归位的会话 id
/// 回填进 `set_session_id`，同一页面接下来的追问就能续接同一会话。
fn attach_stream(
    turn: Turn,
    session_id: Option<String>,
    set_session_id: WriteSignal<Option<String>>,
    on_persisted: Callback<()>,
    on_activity: Callback<()>,
) {
    let mut req = AskRequest::minimal(turn.question.get_value(), turn.ticker.get_untracked());
    req.session_id = session_id;

    let on_event = move |event: ResearchStreamEvent| {
        if !turn.status.get_untracked().is_streaming() {
            return; // 已取消/完成/失败，忽略迟到事件
        }
        match event {
            ResearchStreamEvent::Final(f) => {
                if f.response.session_id.is_some() {
                    set_session_id.set(f.response.session_id.clone());
                }
                turn.handle.set_value(None);
                turn.status.set(TurnStatus::Done(f.response));
                on_persisted.call(());
                on_activity.call(());
                return;
            }
            ResearchStreamEvent::Compare(c) => {
                // 对比结果一次性到达；暂不落库，所以不触发 on_persisted。
                turn.ticker.set(format!(
                    "{} vs {}",
                    c.response.primary.ticker, c.response.peer.ticker
                ));
                turn.handle.set_value(None);
                turn.status
                    .set(TurnStatus::CompareDone(Box::new(c.response)));
                on_activity.call(());
                return;
            }
            ResearchStreamEvent::Error(e) => {
                turn.handle.set_value(None);
                turn.status.set(TurnStatus::Failed(e.message));
                on_activity.call(());
                return;
            }
            _ => {}
        }
        turn.status.update(|status| {
            let TurnStatus::Streaming {
                stage,
                meta_route,
                meta_valuation,
                meta_completeness,
                meta_sources,
                meta_earnings,
                delta_text,
                guard,
            } = status
            else {
                return;
            };
            match event {
                ResearchStreamEvent::Meta(m) => {
                    // 服务端从问题里识别出的主体回填到本轮——气泡标签与后续追问都用它。
                    if turn.ticker.get_untracked().is_empty() {
                        turn.ticker.set(m.ticker);
                    }
                    *meta_route = Some(m.route);
                    *meta_valuation = Some(m.valuation);
                    *meta_completeness = Some(m.data_completeness);
                    *meta_sources = m.connected_sources;
                    *meta_earnings = m.earnings;
                }
                ResearchStreamEvent::Stage(s) => *stage = Some(s),
                ResearchStreamEvent::Delta(d) => delta_text.push_str(&d.text),
                ResearchStreamEvent::Guard(g) => *guard = g.fact_guard,
                ResearchStreamEvent::Final(_)
                | ResearchStreamEvent::Compare(_)
                | ResearchStreamEvent::Error(_) => unreachable!(),
            }
        });
        on_activity.call(());
    };

    let on_error = move |message: String| {
        if turn.status.get_untracked().is_streaming() {
            turn.handle.set_value(None);
            turn.status.set(TurnStatus::Failed(message));
        }
    };

    let handle = api::post_stream("/api/ask/stream", &req, on_event, on_error);
    schedule_turn_timeout(
        turn,
        STREAM_TIMEOUT_MS,
        "研究响应超时（120 秒无返回），请重试。",
    );
    if turn.status.get_untracked().is_streaming() {
        turn.handle.set_value(Some(handle));
    }
}

/// 深度报告的实际请求发送——非流式单请求，供首次提交与重试共用。
fn fire_report_request(
    turn: Turn,
    session_id: Option<String>,
    set_session_id: WriteSignal<Option<String>>,
    on_persisted: Callback<()>,
    on_activity: Callback<()>,
) {
    let mut req = AskRequest::minimal(turn.question.get_value(), turn.ticker.get_untracked());
    req.session_id = session_id;
    schedule_turn_timeout(
        turn,
        REPORT_TIMEOUT_MS,
        "深度报告生成超时（4 分钟无返回），请重试。",
    );
    leptos::spawn_local(async move {
        let outcome = api::post::<_, ReportGenerateResponse>("/api/report/generate", &req).await;
        if !matches!(turn.status.get_untracked(), TurnStatus::ReportPending) {
            return; // 已超时/已放弃等待，迟到结果不覆盖终态
        }
        match outcome {
            Ok(response) => {
                if response.session_id.is_some() {
                    set_session_id.set(response.session_id.clone());
                }
                turn.status.set(TurnStatus::ReportDone(Box::new(response)));
                on_persisted.call(());
            }
            Err(message) => turn.status.set(TurnStatus::Failed(message)),
        }
        on_activity.call(());
    });
}

/// 重试：把已存在的 turn（取消/失败/完成态）原地重置，而不是追加新 turn——按 `is_report`
/// 分流回原来的通道（SSE 问答 或 一次性深度报告），不会把报告失败重试成问答。
fn restart_turn(
    turn: Turn,
    session_id: Option<String>,
    set_session_id: WriteSignal<Option<String>>,
    on_persisted: Callback<()>,
    on_activity: Callback<()>,
) {
    turn.handle.set_value(None);
    if turn.is_report {
        turn.status.set(TurnStatus::ReportPending);
        fire_report_request(turn, session_id, set_session_id, on_persisted, on_activity);
    } else {
        turn.status.set(TurnStatus::streaming_default());
        attach_stream(turn, session_id, set_session_id, on_persisted, on_activity);
    }
}

/// 超时态——一轮在固定窗口内没到终态，视为卡死，主动取消并转失败可重试。
#[cfg(target_arch = "wasm32")]
fn schedule_turn_timeout(turn: Turn, timeout_ms: i32, message: &'static str) {
    use wasm_bindgen::JsCast;
    use wasm_bindgen::prelude::Closure;

    let closure = Closure::once(move || {
        if !turn.status.get_untracked().is_busy() {
            return;
        }
        if let Some(handle) = turn.handle.get_value() {
            handle.cancel();
        }
        turn.handle.set_value(None);
        turn.status.set(TurnStatus::Failed(message.to_string()));
    });
    let _ = leptos::window().set_timeout_with_callback_and_timeout_and_arguments_0(
        closure.as_ref().unchecked_ref(),
        timeout_ms,
    );
    closure.forget();
}

#[cfg(not(target_arch = "wasm32"))]
fn schedule_turn_timeout(_turn: Turn, _timeout_ms: i32, _message: &'static str) {}

// ── Components ────────────────────────────────────────────────────────────

/// 估值三段带（bear / base / bull）。
#[component]
pub(crate) fn ValuationBand(v: ValuationView) -> impl IntoView {
    if let Some(reason) = v.cannot_value_reason.clone() {
        return view! {
            <div class="valuation-block">
                <div class="valuation-head"><span>"估值区间"</span></div>
                <p class="val-none">"未核到 · " {reason}</p>
            </div>
        }
        .into_view();
    }
    view! {
        <div class="valuation-block">
            <div class="valuation-head">
                <span>"估值区间"</span>
                <em>{v.method.clone()}</em>
            </div>
            <div class="val-bands">
                <div class="val-cell">
                    <span class="val-k">"熊"</span>
                    <span class="val-v">{decimal_text(v.bear)}</span>
                </div>
                <div class="val-cell base-cell">
                    <span class="val-k">"基准"</span>
                    <span class="val-v">{decimal_text(v.base)}</span>
                </div>
                <div class="val-cell">
                    <span class="val-k">"牛"</span>
                    <span class="val-v">{decimal_text(v.bull)}</span>
                </div>
            </div>
            {v.upside.clone().map(|u| view! {
                <p class="val-upside">"相对现价 " <strong>{u}</strong></p>
            })}
        </div>
    }
    .into_view()
}

/// 路由意图 / 深度 / 置信度三个 chip——meta 到达即可展示，final 到达后原样复用。
#[component]
pub(crate) fn RouteChips(route: RouteView) -> impl IntoView {
    let conf = (route.confidence * 100.0).round() as u32;
    view! {
        <div class="ac-chips">
            <span class="ac-chip">{intent_label(&route.intent).to_string()}</span>
            <span class="ac-chip dim">{format::depth_label(&route.depth).to_string()}</span>
            <span class="ac-chip dim">"置信 " {conf} "%"</span>
        </div>
    }
}

#[component]
pub(crate) fn CompletenessRow(completeness: u8) -> impl IntoView {
    view! {
        <div class="completeness-row">
            <div
                class="completeness-bar"
                role="progressbar"
                aria-valuenow=completeness
                aria-valuemin="0"
                aria-valuemax="100"
                aria-label="数据完备度"
            >
                <span class="completeness-fill" style=move || format!("width:{completeness}%")></span>
            </div>
            <span class="completeness-label">"数据完备度 " {completeness} "%"</span>
        </div>
    }
}

#[component]
pub(crate) fn DataSources(sources: Vec<String>) -> impl IntoView {
    if sources.is_empty() {
        return ().into_view();
    }
    view! {
        <div class="data-sources">
            {sources.into_iter().map(|s| view! {
                <span class="data-source">{s}</span>
            }).collect_view()}
        </div>
    }
    .into_view()
}

/// 网页证据来源卡——可点击的二手来源列表（标题/域名/日期）。空列表不渲染。二手信息只做
/// 定性支撑，与数字护栏互补：护栏保证数字，来源卡让定性判断可追溯到原始网页。
#[component]
pub(crate) fn SourceCards(sources: Vec<EvidenceView>) -> impl IntoView {
    if sources.is_empty() {
        return ().into_view();
    }
    view! {
        <div class="source-cards">
            <p class="source-cards-title">"网页证据 · " {sources.len()} " 条（二手来源，仅定性支撑）"</p>
            {sources.into_iter().map(|s| {
                let domain = s.source_domain.clone().unwrap_or_else(|| "来源".to_string());
                let meta = match s.published_date.clone() {
                    Some(date) => format!("{domain} · {date}"),
                    None => domain,
                };
                view! {
                    <a class="source-card" href=s.url target="_blank" rel="noopener noreferrer">
                        <span class="source-card-title">{s.title}</span>
                        <span class="source-card-meta">{meta}</span>
                    </a>
                }
            }).collect_view()}
        </div>
    }
    .into_view()
}

/// 定性引用护栏徽标——与数字护栏并列，核的是"定性论断有没有标注真实来源号"。虚构来源号
/// 标红。仅本轮有网页证据时出现。
#[component]
pub(crate) fn CitationBadge(citation: CitationGuardView) -> impl IntoView {
    let cls = if citation.has_hard_fail {
        "fact-guard has-hard"
    } else {
        "fact-guard"
    };
    view! {
        <div class=cls>
            <span class="fact-guard-k">"引用护栏"</span>
            <span>
                "证据 " {citation.evidence_count} " · 已引 " {citation.cited_count}
                {(citation.out_of_range > 0).then(|| view! { <span>" · 虚构 " {citation.out_of_range}</span> })}
            </span>
            {(!citation.note.is_empty()).then(|| view! {
                <p class="fact-guard-note">{citation.note.clone()}</p>
            })}
        </div>
    }
}

#[component]
pub(crate) fn GuardBadge(guard: GuardView) -> impl IntoView {
    let cls = if guard.has_hard_fail {
        "fact-guard has-hard"
    } else {
        "fact-guard"
    };
    view! {
        <div class=cls>
            <span class="fact-guard-k">"数字护栏"</span>
            <span>"核 " {guard.total} " · 过 " {guard.pass} " · 软 " {guard.soft} " · 硬 " {guard.hard}</span>
            {(!guard.soft_note.is_empty()).then(|| view! {
                <p class="fact-guard-note">{guard.soft_note.clone()}</p>
            })}
        </div>
    }
}

/// 下次财报日徽标——`None` 字段即不展示对应行，绝不占位。
#[component]
fn EarningsBadge(earnings: EarningsCalendarView) -> impl IntoView {
    let Some(next_date) = earnings.next_date.clone() else {
        return ().into_view();
    };
    let period = match (earnings.year, earnings.quarter) {
        (Some(year), Some(quarter)) => Some(format!("{year} Q{quarter}")),
        _ => None,
    };
    view! {
        <div class="earnings-badge">
            <span class="earnings-k">"下次财报"</span>
            <span class="earnings-v">{next_date}</span>
            {period.map(|p| view! { <span class="earnings-period">{p}</span> })}
        </div>
    }
    .into_view()
}

/// 作答来源的用户可读中文标签——纯 UI 展示映射，接口层仍传英文枚举值。
const fn answer_source_label(source: AnswerSource) -> &'static str {
    match source {
        AnswerSource::Draft => "结构化草稿",
        AnswerSource::Generated => "模型生成",
        AnswerSource::Unavailable => "未作答",
    }
}

/// 深度报告生成方式的中文标签——同上，仅 UI 展示。
const fn report_mode_label(mode: ReportMode) -> &'static str {
    match mode {
        ReportMode::Model => "模型生成",
        ReportMode::Local => "本地模板兜底",
    }
}

/// 证据摘要文案——流式条与折叠面板共用同一套口径，两者高度一致，落地时不产生跳动。
/// 只汇总真实到手的字段，缺的既不出现在摘要也不出现在面板里。
fn evidence_summary(
    completeness: Option<u8>,
    sources_len: usize,
    guard: Option<&GuardView>,
    citation: Option<&CitationGuardView>,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(value) = completeness {
        parts.push(format!("完备度 {value}%"));
    }
    if sources_len > 0 {
        parts.push(format!("{sources_len} 个数据源"));
    }
    if let Some(g) = guard {
        parts.push(format!("护栏 过 {}/{}", g.pass, g.total));
    }
    if let Some(c) = citation {
        parts.push(format!("引用 {}/{}", c.cited_count, c.evidence_count));
    }
    if parts.is_empty() {
        "研究依据".to_string()
    } else {
        format!("研究依据 · {}", parts.join(" · "))
    }
}

/// 折叠的「研究依据」面板——答案文本永远优先，估值带/完备度/来源/护栏收进一行摘要，
/// 点开展开。
#[component]
fn EvidencePanel(
    valuation: Option<ValuationView>,
    completeness: Option<u8>,
    earnings: Option<EarningsCalendarView>,
    sources: Vec<String>,
    guard: Option<GuardView>,
    #[prop(optional_no_strip)] citation: Option<CitationGuardView>,
    answer_source: Option<String>,
) -> impl IntoView {
    let has_content = valuation.is_some()
        || completeness.is_some()
        || earnings.is_some()
        || !sources.is_empty()
        || guard.is_some()
        || citation.is_some();
    if !has_content {
        return ().into_view();
    }
    let guard_hard = guard.as_ref().is_some_and(|g| g.has_hard_fail)
        || citation.as_ref().is_some_and(|c| c.has_hard_fail);
    let summary_text = evidence_summary(
        completeness,
        sources.len(),
        guard.as_ref(),
        citation.as_ref(),
    );
    view! {
        <details class=if guard_hard { "evidence-panel has-hard" } else { "evidence-panel" }>
            <summary>
                <span class="evidence-summary-text">{summary_text}</span>
                <svg class="evidence-chevron" viewBox="0 0 12 12" fill="none" xmlns="http://www.w3.org/2000/svg" aria-hidden="true">
                    <path d="M3 4.5 6 7.5 9 4.5" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"/>
                </svg>
            </summary>
            <div class="evidence-body">
                {valuation.map(|v| view! { <ValuationBand v=v /> })}
                {completeness.map(|c| view! { <CompletenessRow completeness=c /> })}
                {earnings.map(|e| view! { <EarningsBadge earnings=e /> })}
                <DataSources sources=sources />
                {guard.map(|g| view! { <GuardBadge guard=g /> })}
                {citation.map(|c| view! { <CitationBadge citation=c /> })}
                {answer_source.map(|s| view! { <p class="evidence-provenance">"作答来源 · " {s}</p> })}
            </div>
        </details>
    }
    .into_view()
}

/// 流式进行中的证据摘要条——和落地后的折叠面板同一行高、同一文案口径，但不可展开。
/// 流式期间每个 token 都重渲染这张卡，`<details>` 的展开状态会被反复重置，所以这里
/// 刻意不给交互，等落地后再变成真正可展开的面板。
#[component]
fn EvidenceLiveStrip(
    completeness: Option<u8>,
    sources_len: usize,
    guard: Option<GuardView>,
) -> impl IntoView {
    if completeness.is_none() && sources_len == 0 && guard.is_none() {
        return ().into_view();
    }
    let summary_text = evidence_summary(completeness, sources_len, guard.as_ref(), None);
    view! {
        <div class="evidence-panel" aria-live="polite">
            <div class="evidence-live-strip">
                <span class="evidence-summary-text">{summary_text}</span>
                <span class="evidence-live-hint">"作答完成后可展开"</span>
            </div>
        </div>
    }
    .into_view()
}

/// 流式进行中的卡片：阶段提示 + 从左到右的流动 + 打字机增量。
/// 骨架不铺在答案上方——答案的位置从第一帧就固定。
#[component]
fn StreamingCard(
    stage: Option<ResearchStreamStage>,
    meta_route: Option<RouteView>,
    meta_completeness: Option<u8>,
    meta_sources_len: usize,
    delta_text: String,
    guard: Option<GuardView>,
) -> impl IntoView {
    let has_text = !delta_text.is_empty();
    let html = has_text.then(|| crate::markdown::render(&delta_text));
    let progress_width = stage
        .as_ref()
        .filter(|stage| stage.index > 0 && stage.total > 0)
        .map(|stage| {
            let percent = stage.index.saturating_mul(100) / stage.total;
            format!("width: {percent}%")
        })
        .unwrap_or_else(|| "width: 8%".to_string());
    let current_stage_label = stage_label(stage.as_ref());
    view! {
        <div class="answer-card">
            {meta_route.clone().map(|route| view! {
                <div class="answer-head"><RouteChips route=route /></div>
            })}

            <div class="answer-text-section">
                <p class="stage-label" aria-live="polite">
                    <span class="stage-dot" aria-hidden="true"></span>
                    {current_stage_label}
                    <span class="thinking-wave" aria-hidden="true">
                        <i></i><i></i><i></i><i></i><i></i>
                    </span>
                </p>
                <div
                    class="stage-progress"
                    role="progressbar"
                    aria-label="研究进度"
                >
                    <span class="stage-progress-fill" style=progress_width></span>
                </div>
                {match html {
                    Some(html) => view! { <div class="answer-text is-streaming" inner_html=html></div> }.into_view(),
                    None => view! {
                        <div class="thinking-skeleton" aria-hidden="true"><i></i><i></i><i></i></div>
                    }.into_view(),
                }}
            </div>

            <EvidenceLiveStrip
                completeness=meta_completeness
                sources_len=meta_sources_len
                guard=guard
            />
        </div>
    }
}

/// 答案上的动作条：复制原文、重新生成。
#[component]
fn AnswerActions(text: Option<String>, on_regenerate: Callback<()>) -> impl IntoView {
    let (copied, set_copied) = create_signal(false);
    view! {
        <div class="answer-actions">
            {text.map(|text| view! {
                <button
                    class=move || if copied.get() { "answer-action is-done" } else { "answer-action" }
                    title="复制答案原文"
                    on:click=move |_| {
                        api::copy_text(&text);
                        set_copied.set(true);
                    }
                >
                    {move || if copied.get() {
                        view! { <Icon name="check" /> }
                    } else {
                        view! { <Icon name="copy" /> }
                    }}
                    {move || if copied.get() { "已复制" } else { "复制" }}
                </button>
            })}
            <button
                class="answer-action"
                title="用同一个问题重新研究一次"
                on:click=move |_| on_regenerate.call(())
            >
                <Icon name="refresh" />"重新生成"
            </button>
        </div>
    }
}

/// 干净完成后的答案卡——答案文本优先，估值/完备度/来源/护栏全部收进折叠的证据面板。
#[component]
fn DoneCard(res: AskResponse, on_regenerate: Callback<()>) -> impl IntoView {
    let answer_text = res.answer.clone();
    view! {
        <div class="answer-card">
            <div class="answer-head">
                <RouteChips route=res.route.clone() />
            </div>

            <div class="answer-text-section">
                {match res.answer.clone() {
                    Some(text) => {
                        let html = crate::markdown::render(&text);
                        view! { <div class="answer-text" inner_html=html></div> }.into_view()
                    }
                    None => view! {
                        <p class="answer-unavailable">
                            "未核到模型作答（未配 provider）——本轮只给结构化事实，不臆造。"
                        </p>
                    }.into_view(),
                }}
            </div>

            <AnswerActions text=answer_text on_regenerate=on_regenerate />

            <SourceCards sources=res.sources.clone() />

            <EvidencePanel
                valuation=Some(res.valuation.clone())
                completeness=Some(res.data_completeness)
                earnings=res.earnings.clone()
                sources=res.connected_sources.clone()
                guard=res.fact_guard.clone()
                citation=res.citation_guard.clone()
                answer_source=Some(answer_source_label(res.answer_source).to_string())
            />
        </div>
    }
}

/// 历史会话里的一轮——问题气泡由外层的 thread 渲染，这里只出这一轮的答案。
///
/// 服务端把整段多轮对话存在 `thread_json` 里，此前 UI 只渲染最后一条答案，前面几轮
/// 问答虽然存着却永远看不到，等于把已落库的研究记录丢了一半。现在逐轮原样回放。
/// 路由 chip、完备度、护栏明细当时未存，缺了就不画，不拿假数据充数。
#[component]
fn HistoryCard(turn: ArchivedTurn) -> impl IntoView {
    view! {
        <div class="answer-card">
            {turn.created_at.map(|created| view! {
                <div class="answer-head">
                    <span class="ac-chip dim">"历史记录 · " {created}</span>
                </div>
            })}

            <div class="answer-text-section">
                {match turn.answer {
                    Some(text) => {
                        let html = crate::markdown::render(&text);
                        view! { <div class="answer-text" inner_html=html></div> }.into_view()
                    }
                    None => view! {
                        <p class="answer-unavailable">"该记录未保存作答文本。"</p>
                    }.into_view(),
                }}
            </div>

            {turn.evidence.map(|evidence| view! {
                <EvidencePanel
                    valuation=evidence.valuation
                    completeness=None
                    earnings=None
                    sources=evidence.sources
                    guard=None
                    answer_source=Some("历史存档".to_string())
                />
            })}
        </div>
    }
}

/// 把一条落库的研究会话还原成一串对话轮。
///
/// 有 `thread_json` 就逐轮还原；早期记录没有 thread，退回 `report_markdown` /
/// `full_research` 的单条口径。证据快照只挂最后一轮。
fn archive_to_turns(session: &ResearchSessionDetail, first_id: u64) -> Vec<Turn> {
    let ticker = session.ticker.clone().unwrap_or_default();
    let created = format::timestamp(&session.created_at);
    let evidence = ArchivedEvidence {
        valuation: session
            .decision_panel
            .clone()
            .and_then(|value| serde_json::from_value::<ValuationView>(value).ok()),
        sources: session
            .data_sources
            .as_ref()
            .and_then(|value| value.get("connected").cloned())
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default(),
    };
    let history: Vec<HistoryTurn> = session
        .thread
        .clone()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();

    if history.is_empty() {
        let answer = session
            .report_markdown
            .clone()
            .or_else(|| session.full_research.clone());
        return vec![Turn::new(
            first_id,
            session.question.clone(),
            ticker,
            TurnStatus::Archived(Box::new(ArchivedTurn {
                answer,
                created_at: Some(created),
                evidence: Some(evidence),
            })),
            false,
        )];
    }

    let last = history.len() - 1;
    history
        .into_iter()
        .enumerate()
        .map(|(index, item)| {
            Turn::new(
                first_id + index as u64,
                item.question,
                ticker.clone(),
                TurnStatus::Archived(Box::new(ArchivedTurn {
                    answer: Some(item.answer),
                    created_at: (index == 0).then(|| created.clone()),
                    evidence: (index == last).then(|| evidence.clone()),
                })),
                false,
            )
        })
        .collect()
}

/// 深度报告生成中——非流式单请求，无逐字增量，只给一个进行中提示。
#[component]
fn ReportPendingCard() -> impl IntoView {
    view! {
        <div class="answer-card">
            <div class="answer-head">
                <span class="ac-chip">"深度报告"</span>
            </div>
            <div class="answer-text-section">
                <p class="stage-label" aria-live="polite">
                    <span class="stage-dot" aria-hidden="true"></span>
                    "正在生成深度报告（固定七段结构，通常需要 1–3 分钟）…"
                    <span class="thinking-wave" aria-hidden="true">
                        <i></i><i></i><i></i><i></i><i></i>
                    </span>
                </p>
                <div class="thinking-skeleton" aria-hidden="true"><i></i><i></i><i></i></div>
            </div>
        </div>
    }
}

/// 深度报告完成态——固定七段结构的 Markdown + 复用的估值带/护栏，外加客户端导出。
#[component]
fn ReportCard(res: ReportGenerateResponse, on_regenerate: Callback<()>) -> impl IntoView {
    let html = crate::markdown::render(&res.markdown);
    let filename = format!("{}-深度报告.md", res.ticker);
    let markdown = res.markdown.clone();
    let copy_text = res.markdown.clone();
    let download =
        move |_| api::download_text_file(&filename, "text/markdown;charset=utf-8", &markdown);
    view! {
        <div class="answer-card">
            <div class="answer-head">
                <span class="ac-chip">"深度报告"</span>
                <RouteChips route=res.route.clone() />
            </div>

            <div class="answer-text-section">
                <div class="answer-text" inner_html=html></div>
            </div>

            <div class="answer-actions">
                <button class="answer-action" title="下载 Markdown" on:click=download>
                    <Icon name="download" />"下载 Markdown"
                </button>
                <AnswerActions text=Some(copy_text) on_regenerate=on_regenerate />
            </div>

            <EvidencePanel
                valuation=Some(res.valuation.clone())
                completeness=None
                earnings=res.earnings.clone()
                sources=Vec::new()
                guard=res.fact_guard.clone()
                citation=res.citation_guard.clone()
                answer_source=Some(report_mode_label(res.mode).to_string())
            />
        </div>
    }
}

/// 对话内双主体对比卡——结论优先，两腿证据（估值/完备度/来源/护栏）双栏排在下方，
/// 每腿一个独立折叠面板，绝不把两腿数字混进同一个面板。
#[component]
fn CompareCard(res: CompareResponse, on_regenerate: Callback<()>) -> impl IntoView {
    let answer_html = res
        .answer
        .clone()
        .map(|text| crate::markdown::render(&text));
    let answer_text = res.answer.clone();
    view! {
        <div class="answer-card">
            <div class="answer-head">
                <span class="ac-chip">"双主体对比"</span>
                <RouteChips route=res.route.clone() />
            </div>

            <div class="answer-text-section">
                {match answer_html {
                    Some(html) => view! { <div class="answer-text" inner_html=html></div> }.into_view(),
                    None => view! {
                        <p class="answer-unavailable">
                            "未核到模型作答（未配 provider）——仅给两腿结构化事实，不臆造。"
                        </p>
                    }.into_view(),
                }}
            </div>

            <AnswerActions text=answer_text on_regenerate=on_regenerate />

            <div class="compare-columns">
                <CompareLeg leg=res.primary.clone() />
                <CompareLeg leg=res.peer.clone() />
            </div>

            <p class="compare-note">"两腿独立取数、独立护栏核对；对比结果暂不写入研究历史。"</p>
        </div>
    }
}

/// 对比单腿——ticker 标签 + 该腿自己的证据。
#[component]
fn CompareLeg(leg: CompareLegView) -> impl IntoView {
    view! {
        <div class="compare-leg">
            <p class="compare-leg-ticker">{leg.ticker.clone()}</p>
            <ValuationBand v=leg.valuation.clone() />
            <CompletenessRow completeness=leg.data_completeness />
            <DataSources sources=leg.connected_sources.clone() />
            <SourceCards sources=leg.sources.clone() />
            {leg.fact_guard.clone().map(|g| view! { <GuardBadge guard=g /> })}
        </div>
    }
}

#[component]
fn RetryableMessage(
    message: String,
    cancelled: bool,
    is_report: bool,
    on_retry: Callback<()>,
) -> impl IntoView {
    let label = match (cancelled, is_report) {
        // 深度报告是非流式请求：本地放弃等待不代表服务端停止了生成，说清楚这一点。
        (true, true) => "已停止等待（服务端可能仍在生成，稍后可在研究历史里查看）",
        (true, false) => "已取消",
        (false, _) => "请求未成功",
    };
    view! {
        <div class="answer-card">
            <p class="echo-error">{label} {(!cancelled).then(|| view! { "：" {message.clone()} })}</p>
            <button class="stream-retry" on:click=move |_| on_retry.call(())>
                <Icon name="refresh" />"重试"
            </button>
        </div>
    }
}

// ── App root ──────────────────────────────────────────────────────────────

/// 会话历史侧栏——列表/切换/删除，选中项由 `active_id` 驱动高亮。
#[component]
fn HistorySidebar(
    sessions: Resource<(), Result<ResearchSessionsResponse, String>>,
    active_id: Option<String>,
    on_select: Callback<Option<String>>,
    on_delete: Callback<String>,
    collapsed: ReadSignal<bool>,
    set_collapsed: WriteSignal<bool>,
) -> impl IntoView {
    view! {
        <aside
            class=move || if collapsed.get() { "research-sidebar is-collapsed" } else { "research-sidebar" }
            aria-label="研究历史"
        >
            <div class="research-sidebar-head">
                <button class="sidebar-new" aria-label="创建新研究" on:click=move |_| on_select.call(None)>
                    <span aria-hidden="true"><Icon name="plus" /></span><b>"新建研究对话"</b>
                </button>
                <button
                    class="sidebar-toggle"
                    title=move || if collapsed.get() { "展开研究历史" } else { "收起研究历史" }
                    aria-label=move || if collapsed.get() { "展开研究历史" } else { "收起研究历史" }
                    aria-expanded=move || !collapsed.get()
                    on:click=move |_| set_collapsed.update(|value| *value = !*value)
                ><Icon name="chevron-left" /></button>
            </div>
            <div class="sidebar-section-title">
                <span class="sidebar-title"><b>"研究对话"</b></span>
                <span>"最近"</span>
            </div>
            <div class="research-sidebar-list">
                {move || match sessions.get() {
                    None => crate::workspace::loading_view(),
                    Some(Err(error)) => crate::workspace::error_view(error),
                    Some(Ok(data)) if data.sessions.is_empty() => {
                        crate::workspace::empty_view("还没有研究记录。")
                    }
                    Some(Ok(data)) => {
                        let active_id = active_id.clone();
                        data.sessions.into_iter().map(|item| {
                            let is_active = active_id.as_deref() == Some(item.id.as_str());
                            let go_id = item.id.clone();
                            let del_id = item.id.clone();
                            let ticker = item.ticker.clone().unwrap_or_default();
                            let updated = format::timestamp(&item.updated_at);
                            let title = item.title.clone();
                            view! {
                                <div class=if is_active { "session-item is-active" } else { "session-item" }>
                                    <button
                                        class="session-item-main"
                                        title=item.title.clone()
                                        on:click=move |_| on_select.call(Some(go_id.clone()))
                                    >
                                        <span class="session-item-title">{item.title.clone()}</span>
                                        <span class="session-item-meta">
                                            {(!ticker.is_empty()).then(|| view! { <b>{ticker.clone()}</b> })}
                                            <span>{updated.clone()}</span>
                                        </span>
                                    </button>
                                    <button
                                        class="session-item-delete"
                                        title="删除这条研究记录"
                                        aria-label=format!("删除研究记录 {title}")
                                        on:click=move |ev| {
                                            ev.stop_propagation();
                                            let id = del_id.clone();
                                            confirm_destructive(
                                                "删除研究记录",
                                                "这条研究会话的全部问答与证据将被删除，无法恢复。",
                                                "删除记录",
                                                Callback::new(move |_| on_delete.call(id.clone())),
                                            );
                                        }
                                    ><Icon name="trash" /></button>
                                </div>
                            }
                        }).collect_view()
                    }
                }}
            </div>
        </aside>
    }
}

/// 提交走哪条通道——常规问答（SSE）还是一次性深度报告生成。
#[derive(Clone, Copy, PartialEq, Eq)]
enum SubmitMode {
    Ask,
    Report,
}

/// 让 textarea 随内容长高；上限由 CSS 的 `max-height` 兜住，超出后内部滚动。
#[cfg(target_arch = "wasm32")]
fn autosize(node: &web_sys::HtmlTextAreaElement) {
    let style = node.style();
    let _ = style.set_property("height", "auto");
    let _ = style.set_property("height", &format!("{}px", node.scroll_height()));
}

/// 提交后把编辑器收回一行高。
#[cfg(target_arch = "wasm32")]
fn reset_height(node: &web_sys::HtmlTextAreaElement) {
    let _ = node.style().set_property("height", "auto");
}

/// 窄屏（研究历史在这个宽度下是覆盖式抽屉）默认收起侧栏——否则用户一进研究页
/// 看到的是一整屏历史列表，而不是研究台本身。
#[cfg(target_arch = "wasm32")]
fn narrow_viewport() -> bool {
    leptos::window()
        .inner_width()
        .ok()
        .and_then(|value| value.as_f64())
        .is_some_and(|width| width <= 760.0)
}

#[cfg(not(target_arch = "wasm32"))]
fn narrow_viewport() -> bool {
    false
}

/// 阅读位置跟随：只有用户本来就贴着底部时才自动滚动，向上翻历史时不把人拽回去。
#[cfg(target_arch = "wasm32")]
fn follow_bottom(node: &web_sys::Element, force: bool) {
    const NEAR_BOTTOM_PX: i32 = 140;
    let distance = node.scroll_height() - node.scroll_top() - node.client_height();
    if force || distance <= NEAR_BOTTOM_PX {
        node.set_scroll_top(node.scroll_height());
    }
}

#[component]
pub fn ResearchPage(
    initial_session: Option<String>,
    #[prop(optional_no_strip)] initial_ticker: Option<String>,
    on_navigate: Callback<Option<String>>,
) -> impl IntoView {
    let (question, set_question) = create_signal(String::new());
    // 研究对象输入——公司名/代码皆可；`resolved` 是唯一可信来源，输入文本变化即失效。
    let (company_query, set_company_query) = create_signal(String::new());
    let (resolved, set_resolved) = create_signal(
        initial_ticker
            .clone()
            .map(|ticker| (ticker.clone(), ticker)),
    );
    let (candidates, set_candidates) = create_signal(Vec::<CompanySearchItem>::new());
    let (search_gen, set_search_gen) = create_signal(0u64);
    let (resolving, set_resolving) = create_signal(false);
    let (resolve_error, set_resolve_error) = create_signal(None::<String>);
    let thread = create_rw_signal(Vec::<Turn>::new());
    let (sidebar_collapsed, set_sidebar_collapsed) = create_signal(narrow_viewport());
    let (next_id, set_next_id) = create_signal(0u64);
    let (session_error, set_session_error) = create_signal(None::<String>);
    // 流式活动计数：delta 不改变 thread 向量，滚动跟随需要一个独立的心跳信号。
    let activity = create_rw_signal(0u64);
    let conversation_ref = create_node_ref::<html::Div>();
    let composer_ref = create_node_ref::<html::Textarea>();
    // 本页面当前续接的研究会话 id——深链带来的历史会话，或本页面第一轮问答落库后
    // 归位的新会话；后续每一轮追问都带上它，让模型能承接代词/实体指代。
    let (current_session_id, set_current_session_id) = create_signal(initial_session.clone());

    let sessions = create_resource(
        || (),
        |_| api::get::<ResearchSessionsResponse>("/api/research/sessions"),
    );

    // 深链带 session id 时拉取该会话详情，回填成 thread 首条（只读历史卡）。
    let fetch_session_id = initial_session.clone();
    let session_detail = create_resource(
        || (),
        move |_| {
            let id = fetch_session_id.clone();
            async move {
                match id {
                    Some(id) => Some(
                        api::get::<ResearchSessionResponse>(&format!(
                            "/api/research/sessions/{id}"
                        ))
                        .await,
                    ),
                    None => None,
                }
            }
        },
    );
    create_effect(move |_| {
        if let Some(Some(result)) = session_detail.get() {
            match result {
                Ok(response) => match response.session {
                    Some(session) => {
                        let first_id = next_id.get_untracked();
                        // 恢复研究对象确认态——续接历史会话的追问不需要重填公司。
                        if let Some(ticker) =
                            session.ticker.clone().filter(|value| !value.is_empty())
                        {
                            set_resolved.set(Some((ticker.clone(), ticker)));
                        }
                        let restored = archive_to_turns(&session, first_id);
                        set_next_id.set(first_id + restored.len() as u64);
                        thread.set(restored);
                    }
                    None => {
                        set_session_error.set(Some("未找到该研究记录，可能已被删除。".to_string()))
                    }
                },
                Err(message) => set_session_error.set(Some(message)),
            }
        }
    });

    let delete_session = create_action(|id: &String| {
        let id = id.clone();
        async move {
            api::delete::<MutationResponse>(&format!("/api/research/sessions/{id}"))
                .await
                .map(|_| id)
        }
    });
    let deleted_active_id = initial_session.clone();
    create_effect(move |_| {
        if let Some(Ok(deleted_id)) = delete_session.value().get() {
            if deleted_active_id.as_deref() == Some(deleted_id.as_str()) {
                // 删的是当前会话——导航到 /research 会整体重挂载 ResearchPage，
                // 那边的 sessions 资源天然会带着最新列表重新拉取，这里不用再 refetch
                // 一次（对即将被销毁的作用域发起 refetch 会在异步结果回来时写已销毁的
                // 信号，炸掉整个响应式运行时）。
                on_navigate.call(None);
            } else {
                sessions.refetch();
            }
        }
    });

    // 任一轮仍在流式研究或深度报告生成中都视为 pending——禁止再次提交，避免并发请求的结果错位。
    let pending = move || thread.get().iter().any(|turn| turn.status.get().is_busy());
    let on_persisted = Callback::new(move |_| sessions.refetch());
    let on_activity = Callback::new(move |_| activity.update(|value| *value += 1));

    // 新消息与流式增量到达时让阅读位置跟到最新内容；用户向上翻历史时不打扰。
    #[cfg(target_arch = "wasm32")]
    {
        let scroll_target = conversation_ref;
        create_effect(move |previous: Option<usize>| {
            let turn_count = thread.get().len();
            let _ = activity.get();
            // 新增一轮时强制贴底（用户刚提交，必须看到自己的消息）。
            let force = previous.is_some_and(|count| turn_count > count);
            request_animation_frame(move || {
                if let Some(node) = scroll_target.get_untracked() {
                    follow_bottom(&node, force);
                }
            });
            turn_count
        });
    }

    // 服务端从问题里识别出主体后（meta 回填了最后一轮的 ticker），若 composer 还没有
    // 确认公司，就把它补成 chip——追问自然续接。只看最后一轮：不许把更早轮次的旧公司
    // 回填到一个正在等服务端识别的新问题上；对比轮（"A vs B"）也不回填。
    create_effect(move |_| {
        let latest = thread.get().last().and_then(|turn| {
            let ticker = turn.ticker.get();
            let ticker = ticker.trim();
            (!ticker.is_empty() && !ticker.contains(" vs ")).then(|| ticker.to_string())
        });
        if let Some(ticker) = latest {
            if resolved.get_untracked().is_none() {
                set_resolved.set(Some((ticker.clone(), ticker)));
            }
        }
    });

    // 研究对象输入变化——本地 DB 候选（便宜）实时查，旧一代请求用 gen 挡掉不覆盖新结果。
    let on_query_input = move |ev| {
        let value = event_target_value(&ev);
        set_company_query.set(value.clone());
        set_resolved.set(None);
        set_resolve_error.set(None);
        let query = value.trim().to_string();
        let generation = search_gen.get() + 1;
        set_search_gen.set(generation);
        if query.is_empty() {
            set_candidates.set(Vec::new());
            return;
        }
        leptos::spawn_local(async move {
            let path = format!(
                "/api/companies/search?q={}&limit=8",
                api::encode_query(&query)
            );
            if let Ok(response) = api::get::<CompanySearchResponse>(&path).await {
                if search_gen.get_untracked() == generation {
                    set_candidates.set(response.companies);
                }
            }
        });
    };

    let select_candidate = move |item: CompanySearchItem| {
        let label = company_display(&item.name_zh, item.name_en.as_deref(), &item.ticker);
        set_company_query.set(label.clone());
        set_resolved.set(Some((item.ticker, label)));
        set_candidates.set(Vec::new());
        set_resolve_error.set(None);
    };

    // 提交后把编辑器高度收回一行——不然清空文本后 textarea 还撑着上一条长问题的高度。
    let reset_composer_height = move || {
        #[cfg(target_arch = "wasm32")]
        if let Some(node) = composer_ref.get_untracked() {
            reset_height(&node);
        }
    };

    // 确认好的候选（点选或 resolve 验证成功）才真正起一轮研究。研究对象在会话内保持
    // 确认态不清空——追问同一家公司是最高频路径，绝不让用户每轮重填；换公司点掉 chip 即可。
    let fire = move |mode: SubmitMode, target_ticker: String, target_label: String| {
        let q = question.get().trim().to_string();
        let q = if q.is_empty() && mode == SubmitMode::Report {
            "生成深度研究报告".to_string()
        } else {
            q
        };
        if q.is_empty() {
            return;
        }
        let id = next_id.get();
        set_next_id.set(id + 1);
        let session_id = current_session_id.get();
        let turn = match mode {
            SubmitMode::Ask => Turn::new(
                id,
                q,
                target_ticker.clone(),
                TurnStatus::streaming_default(),
                false,
            ),
            SubmitMode::Report => Turn::new(
                id,
                q,
                target_ticker.clone(),
                TurnStatus::ReportPending,
                true,
            ),
        };
        thread.update(|turns| turns.push(turn));
        match mode {
            SubmitMode::Ask => attach_stream(
                turn,
                session_id,
                set_current_session_id,
                on_persisted,
                on_activity,
            ),
            SubmitMode::Report => fire_report_request(
                turn,
                session_id,
                set_current_session_id,
                on_persisted,
                on_activity,
            ),
        }
        set_question.set(String::new());
        reset_composer_height();
        // 显式确认过的公司 chip 常驻；主体留给服务端识别时（空 ticker）不放假 chip，
        // 等 meta 回填后由 thread 效应补上。
        if !target_ticker.is_empty() {
            set_resolved.set(Some((target_ticker, target_label)));
        }
        set_company_query.set(String::new());
        set_candidates.set(Vec::new());
    };

    let submit = move |mode: SubmitMode| {
        if pending() || resolving.get() {
            return;
        }
        if mode == SubmitMode::Ask && question.get().trim().is_empty() {
            return;
        }
        if let Some((target_ticker, target_label)) = resolved.get() {
            fire(mode, target_ticker, target_label);
            return;
        }
        let query = company_query.get().trim().to_string();
        if query.is_empty() {
            // 没有显式研究对象——把识别交给服务端（resolve 链跑问题文本；
            // 双主体对比问题也在服务端分流）。识别失败会以流错误诚实返回。
            fire(mode, String::new(), String::new());
            return;
        }
        set_resolving.set(true);
        set_resolve_error.set(None);
        leptos::spawn_local(async move {
            let path = format!("/api/companies/resolve?q={}", api::encode_query(&query));
            let outcome = api::get::<CompanyResolveResponse>(&path).await;
            set_resolving.set(false);
            match outcome {
                Ok(response) => match response.company {
                    Some(company) => {
                        let label = company_display(
                            &company.name_zh,
                            company.name_en.as_deref(),
                            &company.ticker,
                        );
                        fire(mode, company.ticker, label);
                    }
                    None => set_resolve_error.set(Some(format!(
                        "未能把「{query}」识别为可研究的公司，请换个更准确的名称或代码。"
                    ))),
                },
                Err(message) => set_resolve_error.set(Some(message)),
            }
        });
    };

    // 停止生成：作用在当前正在跑的那一轮上。
    let stop_active = move || {
        if let Some(turn) = thread
            .get_untracked()
            .into_iter()
            .find(|turn| turn.status.get_untracked().is_busy())
        {
            turn.cancel();
        }
    };

    let has_thread = move || !thread.get().is_empty();
    let awaiting_session = initial_session.is_some();
    let intent_prompts: [(&str, &str); 5] = [
        ("商业模式", "分析这家公司的商业模式与核心增长驱动"),
        ("盈利质量", "分析这家公司的盈利质量、现金流与会计风险"),
        ("竞争格局", "分析这家公司的竞争格局、护城河与份额变化"),
        ("估值概率", "基于最新基本面给出熊、基准、牛三种估值情景"),
        ("证伪条件", "列出这家公司当前论点最关键、可观察的证伪条件"),
    ];
    // 首屏的高频研究入口：只给公司与问题，不编造"12 条证据"这类没有来源的数字。
    let curated: [(&str, &str, &str, &str); 4] = [
        (
            "腾讯控股",
            "0700.HK",
            "is-tencent",
            "腾讯当前的估值便宜吗？",
        ),
        ("苹果公司", "AAPL", "is-apple", "苹果的盈利质量正在变化吗？"),
        ("英伟达", "NVDA", "is-nvidia", "英伟达的护城河能维持多久？"),
        (
            "阿里巴巴",
            "9988.HK",
            "is-alibaba",
            "什么会证伪阿里巴巴的复苏？",
        ),
    ];

    view! {
        <div class="research-shell">
            <HistorySidebar
                sessions=sessions
                active_id=initial_session.clone()
                on_select=on_navigate
                on_delete=Callback::new(move |id| delete_session.dispatch(id))
                collapsed=sidebar_collapsed
                set_collapsed=set_sidebar_collapsed
            />
        // ── Desk ──
        <main class=move || if has_thread() { "desk has-thread" } else { "desk" }>
            <div class="desk-toolbar">
                <div class="desk-context">
                    <span class="desk-context-mark" aria-hidden="true"></span>
                    <span>
                        <small>{move || if has_thread() { "ACTIVE RESEARCH" } else { "NEW RESEARCH" }}</small>
                        <strong>{move || if has_thread() { "证据研究会话" } else { "开始一段新的研究" }}</strong>
                    </span>
                </div>
                <div class="desk-toolbar-meta">
                    <span class="trust-chip"><i></i>"数字护栏开启"</span>
                </div>
            </div>
            // conversation thread
            <div node_ref=conversation_ref class=move || if has_thread() { "conversation" } else { "conversation is-empty" }>
                {move || if !has_thread() {
                    if let Some(error) = session_error.get() {
                        view! {
                            <div class="echo-empty">
                                <p class="echo-empty-sub form-error">{error}</p>
                            </div>
                        }.into_view()
                    } else if awaiting_session {
                        view! {
                            <div class="echo-empty">
                                <p class="echo-empty-sub">"正在加载历史会话…"</p>
                            </div>
                        }.into_view()
                    } else {
                    // ── 空态 hero ──
                    view! {
                        <div class="echo-empty">
                            <EchoArt class="hero-echo-art" />
                            <div class="hero-heading-row">
                                <h1>
                                    <span class="line-1">"让每一个判断，"</span>
                                    <span class="line-2">"都有证据。"</span>
                                </h1>
                            </div>
                            <div class="research-launch-support">
                                <div class="research-intents" aria-label="研究主题快捷入口">
                                    {intent_prompts.into_iter().map(|(label, prompt)| view! {
                                        <button on:click=move |_| {
                                            set_question.set(prompt.to_string());
                                            #[cfg(target_arch = "wasm32")]
                                            if let Some(node) = composer_ref.get_untracked() {
                                                let _ = node.focus();
                                                autosize(&node);
                                            }
                                        }>{label}</button>
                                    }).collect_view()}
                                </div>
                                <section class="company-showcase-section" aria-labelledby="popular-research-title">
                                    <header class="company-showcase-heading">
                                        <div>
                                            <span class="company-showcase-kicker">"CURATED RESEARCH"</span>
                                            <h2 id="popular-research-title">"常用研究"</h2>
                                        </div>
                                        <p>"从高频判断开始，或在下方直接提出你的问题。"</p>
                                    </header>
                                    <div class="company-showcase">
                                        {curated.into_iter().map(|(name, ticker, logo, prompt)| {
                                            let initial = name.chars().next().unwrap_or('E').to_string();
                                            view! {
                                                <button
                                                    class="company-card"
                                                    aria-label=format!("研究 {name} {ticker}：{prompt}")
                                                    on:click=move |_| {
                                                        set_resolved.set(Some((
                                                            ticker.to_string(),
                                                            format!("{name} · {ticker}"),
                                                        )));
                                                        set_question.set(prompt.to_string());
                                                        #[cfg(target_arch = "wasm32")]
                                                        if let Some(node) = composer_ref.get_untracked() {
                                                            let _ = node.focus();
                                                            autosize(&node);
                                                        }
                                                    }
                                                >
                                                    <span class="company-card-head">
                                                        <i class=format!("company-logo {logo}")>{initial}</i>
                                                        <span><strong>{name}</strong><small>{ticker}</small></span>
                                                    </span>
                                                    <span class="company-question">{prompt}</span>
                                                    <span class="company-evidence">
                                                        <span>"开始研究"</span>
                                                        <b aria-hidden="true">"→"</b>
                                                    </span>
                                                </button>
                                            }
                                        }).collect_view()}
                                    </div>
                                </section>
                            </div>
                        </div>
                    }.into_view()
                    }
                } else {
                    // ── 对话 thread ──
                    // For + key：每一轮只挂载一次，流式增量只重渲染那一轮的卡。
                    view! {
                        <div>
                            <For
                                each=move || thread.get()
                                key=|turn| turn.id
                                children=move |turn| {
                                    let on_retry = Callback::new(move |_| {
                                        restart_turn(
                                            turn,
                                            current_session_id.get_untracked(),
                                            set_current_session_id,
                                            on_persisted,
                                            on_activity,
                                        );
                                    });
                                    view! {
                                        // user bubble——问题为主体，研究对象作为小标签而不是拼接文本
                                        <div class="message user">
                                            <div class="bubble">
                                                {move || {
                                                    let label = turn.ticker.get();
                                                    (!label.is_empty()).then(|| view! {
                                                        <span class="bubble-ticker">{label}</span>
                                                    })
                                                }}
                                                <p class="bubble-text">{turn.question.get_value()}</p>
                                            </div>
                                        </div>
                                        // assistant card
                                        <div class="message">
                                            <div class="bubble assistant-bubble">
                                                {move || match turn.status.get() {
                                                    TurnStatus::Streaming {
                                                        stage, meta_route, meta_completeness,
                                                        meta_sources, delta_text, guard, ..
                                                    } => view! {
                                                        <StreamingCard
                                                            stage=stage
                                                            meta_route=meta_route
                                                            meta_completeness=meta_completeness
                                                            meta_sources_len=meta_sources.len()
                                                            delta_text=delta_text
                                                            guard=guard
                                                        />
                                                    }.into_view(),
                                                    TurnStatus::Done(response) => view! {
                                                        <DoneCard res=response on_regenerate=on_retry />
                                                    }.into_view(),
                                                    TurnStatus::CompareDone(response) => view! {
                                                        <CompareCard res=*response on_regenerate=on_retry />
                                                    }.into_view(),
                                                    TurnStatus::Archived(archived) => view! {
                                                        <HistoryCard turn=*archived />
                                                    }.into_view(),
                                                    TurnStatus::ReportPending => view! { <ReportPendingCard /> }.into_view(),
                                                    TurnStatus::ReportDone(response) => view! {
                                                        <ReportCard res=*response on_regenerate=on_retry />
                                                    }.into_view(),
                                                    TurnStatus::Failed(message) => view! {
                                                        <RetryableMessage
                                                            message=message
                                                            cancelled=false
                                                            is_report=turn.is_report
                                                            on_retry=on_retry
                                                        />
                                                    }.into_view(),
                                                    TurnStatus::Cancelled => view! {
                                                        <RetryableMessage
                                                            message=String::new()
                                                            cancelled=true
                                                            is_report=turn.is_report
                                                            on_retry=on_retry
                                                        />
                                                    }.into_view(),
                                                }}
                                            </div>
                                        </div>
                                    }
                                }
                            />
                        </div>
                    }.into_view()
                }}
            </div>

            // ── Composer（贴底常驻；空态与对话态同一位置，首次提交不跳位）──
            <div class="composer">
                <div class="composer-panel">
                    <textarea
                        node_ref=composer_ref
                        prop:value=question
                        on:input=move |ev| {
                            set_question.set(event_target_value(&ev));
                            #[cfg(target_arch = "wasm32")]
                            if let Some(node) = composer_ref.get_untracked() {
                                autosize(&node);
                            }
                        }
                        on:keydown=move |ev| {
                            if ev.key() == "Enter" && !ev.shift_key() {
                                ev.prevent_default();
                                submit(SubmitMode::Ask);
                            }
                        }
                        placeholder="输入公司名、代码，或直接问出你的判断"
                        aria-label="研究问题"
                        rows="1"
                    />
                    <div class="composer-footer">
                        <div class="company-picker">
                            {move || match resolved.get() {
                                // 已确认研究对象——chip 常驻，追问免重填；点 × 更换公司。
                                Some((_, label)) => view! {
                                    <div class="company-chip">
                                        <span class="company-chip-label" title=label.clone()>{label}</span>
                                        <button
                                            class="company-chip-clear"
                                            title="更换研究对象"
                                            aria-label="更换研究对象"
                                            on:click=move |_| {
                                                set_resolved.set(None);
                                                set_company_query.set(String::new());
                                            }
                                        ><Icon name="close" /></button>
                                    </div>
                                }.into_view(),
                                None => view! {
                                    <input
                                        class="company-input"
                                        prop:value=company_query
                                        on:input=on_query_input
                                        on:keydown=move |ev| {
                                            if ev.key() == "Enter" {
                                                submit(SubmitMode::Ask);
                                            } else if ev.key() == "Escape" && !candidates.get_untracked().is_empty() {
                                                ev.stop_propagation();
                                                set_candidates.set(Vec::new());
                                            }
                                        }
                                        placeholder="研究对象（可留空，自动从问题识别）"
                                        aria-label="研究对象"
                                        disabled=resolving
                                        role="combobox"
                                        aria-expanded=move || !candidates.get().is_empty()
                                        aria-autocomplete="list"
                                    />
                                }.into_view(),
                            }}
                            {move || resolving.get().then(|| view! {
                                <span class="company-status">"核实中…"</span>
                            })}
                            {move || {
                                let items = candidates.get();
                                if items.is_empty() {
                                    view! {}.into_view()
                                } else {
                                    view! {
                                        <div class="company-dropdown" role="listbox">
                                            {items.into_iter().map(|item| {
                                                let label = company_display(&item.name_zh, item.name_en.as_deref(), &item.ticker);
                                                let industry = item.industry.clone();
                                                let pick = item.clone();
                                                view! {
                                                    <button
                                                        type="button"
                                                        class="company-item"
                                                        role="option"
                                                        on:click=move |_| select_candidate(pick.clone())
                                                    >
                                                        <span class="company-item-name">{label}</span>
                                                        {industry.map(|value| view! { <span class="company-item-industry">{value}</span> })}
                                                    </button>
                                                }
                                            }).collect_view()}
                                        </div>
                                    }.into_view()
                                }
                            }}
                        </div>
                        <button
                            class="composer-report"
                            on:click=move |_| submit(SubmitMode::Report)
                            disabled=move || pending() || resolving.get()
                            title="生成固定七段结构的深度研究报告"
                            aria-label="生成深度研究报告"
                        ><span aria-hidden="true"><Icon name="sparkle" /></span>"深度报告"</button>
                        // 生成中同一位置换成停止——用户找刹车不该翻回上面的卡片。
                        {move || if pending() {
                            view! {
                                <button
                                    class="composer-send is-stop"
                                    on:click=move |_| stop_active()
                                    title="停止生成"
                                    aria-label="停止生成"
                                ><Icon name="stop" /></button>
                            }
                        } else {
                            view! {
                                <button
                                    class="composer-send"
                                    on:click=move |_| submit(SubmitMode::Ask)
                                    disabled=move || resolving.get() || question.get().trim().is_empty()
                                    title="发送（Enter）"
                                    aria-label="发送研究请求"
                                ><Icon name="arrow-up" /></button>
                            }
                        }}
                    </div>
                    <div class="feedback-slot" aria-live="polite">
                        {move || resolve_error.get().map(|message| view! {
                            <p class="company-error" role="alert">{message}</p>
                        })}
                    </div>
                </div>
                <div class="composer-meta">
                    <span>"Echo 可能出错，关键结论请结合证据面板核验。"</span>
                    <span class="composer-shortcut"><kbd>"Enter"</kbd>" 发送 · "<kbd>"Shift"</kbd><kbd>"Enter"</kbd>" 换行"</span>
                </div>
            </div>
        </main>
        </div>
    }
}
