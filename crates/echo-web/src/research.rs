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
    AnswerSource, AskRequest, AskResponse, CitationGuardView, CompareLegView, CompareResponse,
    Decimal, EarningsCalendarView, EvidenceView, GuardView, MutationResponse,
    ResearchSessionDetail, ResearchSessionResponse, ResearchSessionsResponse, ResearchStreamEvent,
    ResearchStreamStage, ResearchStreamStageName, RouteView, ValuationView,
};
use leptos::*;

/// 流式研究的整体超时窗口（毫秒）。一次性定时器，不随事件重置：多阶段研究本就该在这个
/// 窗口内跑完，卡死比慢更值得暴露。
const STREAM_TIMEOUT_MS: i32 = 120_000;

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
}

impl Turn {
    fn new(id: u64, question: String, ticker: String, status: TurnStatus) -> Self {
        Self {
            id,
            question: store_value(question),
            ticker: create_rw_signal(ticker),
            status: create_rw_signal(status),
            handle: store_value(None),
        }
    }

    /// 主动取消：先中止底层请求，再落到终态。已是终态就什么都不做。
    fn cancel(&self) {
        if !self.status.get_untracked().is_streaming() {
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

/// 重试 / 重新生成：把已存在的 turn（取消、失败或已完成）原地重置再跑一遍，
/// 而不是追加一条新 turn。
fn restart_turn(
    turn: Turn,
    session_id: Option<String>,
    set_session_id: WriteSignal<Option<String>>,
    on_persisted: Callback<()>,
    on_activity: Callback<()>,
) {
    turn.handle.set_value(None);
    turn.status.set(TurnStatus::streaming_default());
    attach_stream(turn, session_id, set_session_id, on_persisted, on_activity);
}

/// 超时态——一轮在固定窗口内没到终态，视为卡死，主动取消并转失败可重试。
#[cfg(target_arch = "wasm32")]
fn schedule_turn_timeout(turn: Turn, timeout_ms: i32, message: &'static str) {
    use wasm_bindgen::JsCast;
    use wasm_bindgen::prelude::Closure;

    let closure = Closure::once(move || {
        if !turn.status.get_untracked().is_streaming() {
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

/// 路由意图 / 深度 / 置信度三个 chip。
///
/// 这三个 chip 原来常驻在答案卡顶部。那是路由器的内部状态，读答案的人不会因为它改变
/// 任何决定，却每一轮都要先撞见它。现在只出现在折叠的证据面板里——想查依然查得到，
/// 默认的阅读路径上不再有它。
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
    #[prop(optional_no_strip)] route: Option<RouteView>,
    answer_source: Option<String>,
) -> impl IntoView {
    let has_content = valuation.is_some()
        || completeness.is_some()
        || earnings.is_some()
        || !sources.is_empty()
        || guard.is_some()
        || citation.is_some()
        || route.is_some();
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
                {route.map(|r| view! { <RouteChips route=r /> })}
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

/// 答案上的动作条：复制原文、导出 Markdown、重新生成。
///
/// 导出原来只挂在深度报告卡上；报告入口收掉后导出下沉到每一条答案——研究结论要能带走，
/// 这个能力不该跟着一个按钮一起消失。
#[component]
fn AnswerActions(
    text: Option<String>,
    #[prop(into)] ticker: String,
    on_regenerate: Callback<()>,
) -> impl IntoView {
    let (copied, set_copied) = create_signal(false);
    let export = text.clone();
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
            {export.map(|markdown| {
                let filename = if ticker.trim().is_empty() {
                    "echo-研究.md".to_string()
                } else {
                    format!("{}-研究.md", ticker.trim())
                };
                view! {
                    <button
                        class="answer-action"
                        title="导出为 Markdown"
                        on:click=move |_| api::download_text_file(
                            &filename,
                            "text/markdown;charset=utf-8",
                            &markdown,
                        )
                    ><Icon name="download" />"导出"</button>
                }
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

            <AnswerActions text=answer_text ticker=res.ticker.clone() on_regenerate=on_regenerate />

            <SourceCards sources=res.sources.clone() />

            <EvidencePanel
                valuation=Some(res.valuation.clone())
                completeness=Some(res.data_completeness)
                earnings=res.earnings.clone()
                sources=res.connected_sources.clone()
                guard=res.fact_guard.clone()
                citation=res.citation_guard.clone()
                route=Some(res.route.clone())
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
            )
        })
        .collect()
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
            // 不放"双主体对比"标签：下面就是两列并排的证据，结构自己说得清楚。
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

            <AnswerActions
                text=answer_text
                ticker=format!("{}-vs-{}", res.primary.ticker, res.peer.ticker)
                on_regenerate=on_regenerate
            />

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
fn RetryableMessage(message: String, cancelled: bool, on_retry: Callback<()>) -> impl IntoView {
    let label = if cancelled { "已取消" } else { "请求未成功" };
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
    // 当前会话锁定的研究主体。这不是一个让用户填的字段——界面上没有"研究对象"输入框，
    // 主体一律由服务端从问题文本识别（`prepare_research_request` 的 resolve 链），
    // 识别结果回填到这里，后续追问带上它，"那它的估值呢"才能承接到同一家公司。
    let (subject, set_subject) = create_signal(initial_ticker.clone().unwrap_or_default());
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
                        // 恢复主体——续接历史会话的追问要能承接同一家公司。
                        if let Some(ticker) =
                            session.ticker.clone().filter(|value| !value.is_empty())
                        {
                            set_subject.set(ticker);
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

    // 任一轮仍在流式研究中都视为 pending——禁止再次提交，避免并发请求的结果错位。
    let pending = move || thread.get().iter().any(|turn| turn.status.get().is_streaming());
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

    // 服务端识别出主体后（meta 回填了最后一轮的 ticker）把它记下来，后续追问带上去。
    // 只看最后一轮，且对比轮（"A vs B"）不回填——那不是单一主体。
    create_effect(move |_| {
        let latest = thread.get().last().and_then(|turn| {
            let ticker = turn.ticker.get();
            let ticker = ticker.trim();
            (!ticker.is_empty() && !ticker.contains(" vs ")).then(|| ticker.to_string())
        });
        if let Some(ticker) = latest {
            if subject.get_untracked() != ticker {
                set_subject.set(ticker);
            }
        }
    });

    // 提交后把编辑器高度收回一行——不然清空文本后 textarea 还撑着上一条长问题的高度。
    let reset_composer_height = move || {
        #[cfg(target_arch = "wasm32")]
        if let Some(node) = composer_ref.get_untracked() {
            reset_height(&node);
        }
    };

    // 提交一轮研究。主体不由用户填：`subject` 有值（服务端上一轮识别出来的，或从资料库
    // 带过来的）就带上，否则交给服务端从问题文本识别；识别失败会以流错误诚实返回。
    let submit = move || {
        if pending() {
            return;
        }
        let q = question.get().trim().to_string();
        if q.is_empty() {
            return;
        }
        let id = next_id.get();
        set_next_id.set(id + 1);
        let turn = Turn::new(id, q, subject.get(), TurnStatus::streaming_default());
        thread.update(|turns| turns.push(turn));
        attach_stream(
            turn,
            current_session_id.get(),
            set_current_session_id,
            on_persisted,
            on_activity,
        );
        set_question.set(String::new());
        reset_composer_height();
    };

    // 停止生成：作用在当前正在跑的那一轮上。
    let stop_active = move || {
        if let Some(turn) = thread
            .get_untracked()
            .into_iter()
            .find(|turn| turn.status.get_untracked().is_streaming())
        {
            turn.cancel();
        }
    };

    let has_thread = move || !thread.get().is_empty();
    let awaiting_session = initial_session.is_some();
    // 首屏的高频研究入口：只给公司与问题，不编造"12 条证据"这类没有来源的数字。
    let curated: [(&str, &str, &str); 4] = [
        ("腾讯控股", "0700.HK", "腾讯当前的估值便宜吗？"),
        ("苹果公司", "AAPL", "苹果的盈利质量正在变化吗？"),
        ("英伟达", "NVDA", "英伟达的护城河能维持多久？"),
        ("阿里巴巴", "9988.HK", "什么会证伪阿里巴巴的复苏？"),
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
            // 对话区不放状态条——"证据研究会话""数字护栏开启"这类文案对用户不产生任何
            // 决策价值，只是噪声。护栏状态在答案的证据面板里有真实数字。
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
                            // 空态只留一层脚手架：标题 + 四个真实可点的研究入口。
                            // 原来的「研究主题」快捷词（"分析这家公司的…"）没有主体，
                            // 在没有公司选择器之后点了必定识别失败，属于会骗人的入口，一并去掉。
                            <div class="research-launch-support">
                                <section class="company-showcase-section" aria-label="常用研究入口">
                                    <div class="company-showcase">
                                        {curated.into_iter().map(|(name, ticker, prompt)| {
                                            let initial = name.chars().next().unwrap_or('E').to_string();
                                            view! {
                                                <button
                                                    class="company-card"
                                                    aria-label=format!("研究 {name} {ticker}：{prompt}")
                                                    on:click=move |_| {
                                                        set_subject.set(ticker.to_string());
                                                        set_question.set(prompt.to_string());
                                                        #[cfg(target_arch = "wasm32")]
                                                        if let Some(node) = composer_ref.get_untracked() {
                                                            let _ = node.focus();
                                                            autosize(&node);
                                                        }
                                                    }
                                                >
                                                    <span class="company-card-head">
                                                        <i class="company-logo">{initial}</i>
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
                                                        stage, meta_completeness,
                                                        meta_sources, delta_text, guard, ..
                                                    } => view! {
                                                        <StreamingCard
                                                            stage=stage
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
                                                    TurnStatus::Failed(message) => view! {
                                                        <RetryableMessage
                                                            message=message
                                                            cancelled=false
                                                            on_retry=on_retry
                                                        />
                                                    }.into_view(),
                                                    TurnStatus::Cancelled => view! {
                                                        <RetryableMessage
                                                            message=String::new()
                                                            cancelled=true
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

            // ── 编辑器（贴底常驻；空态与对话态同一位置，首次提交不跳位）──
            // 里面只有一个输入框和一个按钮：没有研究对象选择器（主体由服务端识别）、
            // 没有第二条提交通道、没有快捷键说明和免责小字。
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
                                submit();
                            }
                        }
                        placeholder="问一个关于公司的问题"
                        aria-label="研究问题"
                        rows="1"
                    />
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
                                on:click=move |_| submit()
                                disabled=move || question.get().trim().is_empty()
                                title="发送（Enter 发送，Shift + Enter 换行）"
                                aria-label="发送研究请求"
                            ><Icon name="arrow-up" /></button>
                        }
                    }}
                </div>
            </div>
        </main>
        </div>
    }
}
