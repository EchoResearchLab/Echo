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
    AnswerSource, AskRequest, AskResponse, CitationGuardView, CompanyHeaderView, CompareLegView,
    CompareResponse, Decimal, EarningsCalendarView, EvidenceView, GuardView, MutationResponse,
    ReportGenerateResponse, ResearchSessionDetail, ResearchSessionResponse,
    ResearchSessionsResponse, ResearchStreamEvent, ResearchStreamStage, ResearchStreamStageName,
    RouteView, ValuationView,
};
use leptos::*;

/// 流式研究的整体超时窗口（毫秒）。一次性定时器，不随事件重置：多阶段研究本就该在这个
/// 窗口内跑完，卡死比慢更值得暴露。
const STREAM_TIMEOUT_MS: i32 = 120_000;

#[cfg(target_arch = "wasm32")]
fn now_ms() -> f64 {
    js_sys::Date::now()
}

#[cfg(not(target_arch = "wasm32"))]
fn now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |duration| duration.as_secs_f64() * 1_000.0)
}

/// 一轮的终态或进行态。`Streaming` 里的字段随 SSE 事件逐步填充。
#[derive(Clone)]
enum TurnStatus {
    Streaming {
        stage: Option<ResearchStreamStage>,
        delta_text: String,
    },
    Done(Box<AskResponse>),
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
            delta_text: String::new(),
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
    started_at_ms: RwSignal<f64>,
    processed_seconds: RwSignal<Option<u64>>,
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
            started_at_ms: create_rw_signal(now_ms()),
            processed_seconds: create_rw_signal(None),
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

    fn restart_timer(&self) {
        self.started_at_ms.set(now_ms());
        self.processed_seconds.set(None);
    }

    fn finish_processing(&self) {
        let elapsed_ms = (now_ms() - self.started_at_ms.get_untracked()).max(0.0);
        self.processed_seconds
            .set(Some(((elapsed_ms / 1_000.0).ceil() as u64).max(1)));
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

fn stage_activity(name: Option<ResearchStreamStageName>) -> &'static str {
    match name {
        None => "正在理解你的问题与研究目标",
        Some(ResearchStreamStageName::Routing) => "正在理解问题意图与研究深度",
        Some(ResearchStreamStageName::Resolving) => "正在确认公司、证券代码与研究主体",
        Some(ResearchStreamStageName::MarketFinancials) => "正在核对实时行情、财报与关键指标",
        Some(ResearchStreamStageName::Evidence) => "正在检索原始披露、网页证据与反例",
        Some(ResearchStreamStageName::Valuation) => "正在建立估值框架与关键假设",
        Some(ResearchStreamStageName::Generating) => "正在综合证据并组织研究结论",
        Some(ResearchStreamStageName::FactCheck) => "正在核对事实、数字与引用",
        Some(ResearchStreamStageName::Assembling) => "正在整理已经核实的研究事实",
        Some(ResearchStreamStageName::Verifying) => "正在检查数字护栏与结论边界",
        Some(ResearchStreamStageName::Persisting) => "正在保存本轮研究上下文",
    }
}

/// 把会话按天分组，保持服务端给的倒序。返回 `(组标题, 该组会话)`，标题用"今天/昨天/日期"。
///
/// 这里只按 `updated_at` 的日期部分切分，不做任何排序或去重：服务端已按更新时间倒序，
/// 前端再排一次只会在两边口径不一致时产生难查的错位。
fn group_sessions_by_day(
    sessions: Vec<echo_contracts::ResearchSessionSummary>,
) -> Vec<(String, Vec<echo_contracts::ResearchSessionSummary>)> {
    let mut groups: Vec<(String, Vec<_>)> = Vec::new();
    for item in sessions {
        let label = format::day_label(&item.updated_at);
        match groups.last_mut() {
            Some((last, bucket)) if *last == label => bucket.push(item),
            _ => groups.push((label, vec![item])),
        }
    }
    groups
}

/// 数据完备度的研究语言表述。设计系统禁止"完备度 xx%"这类产品状态词——读研究结论的人
/// 要知道的是"这个判断有多少事实支撑"，不是一个进度条读数。
fn completeness_phrase(completeness: u8) -> &'static str {
    match completeness {
        80..=u8::MAX => "主要事实已核到",
        50..=79 => "部分事实未核到，置信度下降",
        _ => "多数事实未核到，仅供定性参考",
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
                turn.finish_processing();
                turn.status.set(TurnStatus::Done(Box::new(f.response)));
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
                turn.finish_processing();
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
        match event {
            ResearchStreamEvent::Meta(m) => {
                // 服务端识别出的主体只用于续接会话；思考界面只显示当前真实阶段。
                if turn.ticker.get_untracked().is_empty() {
                    turn.ticker.set(m.ticker);
                }
            }
            ResearchStreamEvent::Stage(next) => {
                turn.status.update(|status| {
                    if let TurnStatus::Streaming { stage, .. } = status {
                        *stage = Some(next);
                    }
                });
            }
            ResearchStreamEvent::Delta(delta) => {
                turn.status.update(|status| {
                    if let TurnStatus::Streaming { delta_text, .. } = status {
                        delta_text.push_str(&delta.text);
                    }
                });
            }
            ResearchStreamEvent::Guard(_) => {}
            ResearchStreamEvent::Final(_)
            | ResearchStreamEvent::Compare(_)
            | ResearchStreamEvent::Error(_) => unreachable!(),
        }
        // 流式 delta 只增长当前答案，不再每个 token 强制贴底。逐字滚动会让整张页面
        // 连续上移，用户看到的是抖动而不是流畅生成；终态与新消息仍会正常归位。
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
    turn.restart_timer();
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

/// 估值带与现价在同一条轴上的位置：`(带左缘%, 带宽%, 现价%)`。全程 Decimal，不碰浮点。
///
/// 轴的范围取 `min(熊,现价) … max(牛,现价)` 再留 8% 余量，**不是**固定的熊–牛。原因：现价
/// 高于牛（或低于熊）恰恰是最该看清的情形，若把轴固定成熊–牛，现价只能溢出到轴外——实测
/// AAPL 现价 324.98 对牛 272.01 会被推到 112% 处，标记直接飘到卡片外的背景上。夹到端点也
/// 不行：那样"贵一点点"和"贵一倍"长得一模一样，而这正是这条轴唯一要回答的问题。
/// 让轴自适应后，带子成为轴上的一段，现价与它的距离就是"贵/便宜多少"的直观表达。
fn band_geometry(bear: Decimal, bull: Decimal, price: Decimal) -> Option<(String, String, String)> {
    let lo_raw = bear.min(price);
    let hi_raw = bull.max(price);
    let pad = (hi_raw - lo_raw) * Decimal::new(8, 2);
    let (lo, hi) = (lo_raw - pad, hi_raw + pad);
    let span = hi - lo;
    if span <= Decimal::ZERO {
        return None;
    }
    let pct = |v: Decimal| (v - lo) * Decimal::ONE_HUNDRED / span;
    let left = pct(bear);
    let width = pct(bull) - left;
    let render = |v: Decimal| v.round_dp(2).normalize().to_string();
    Some((render(left), render(width), render(pct(price))))
}

/// 行情抬头——答案上方"研究的是哪家、现在多少钱"。
///
/// 放在答案之上而非折叠面板里：读一段估值判断时，"现价多少"是理解它的前提，不该要展开
/// 才看得到。每个字段各自可缺，缺的就不画——一行只剩代码时整个抬头不渲染（见后端
/// `company_header`），绝不用 0 占位。
#[component]
pub(crate) fn CompanyHeader(c: CompanyHeaderView) -> impl IntoView {
    let name = c.name.clone().unwrap_or_else(|| c.ticker.clone());
    let show_ticker = c.name.is_some();
    let currency = c.currency.clone().unwrap_or_default();
    // 涨跌方向决定语义色；缺涨跌幅时不着色，也不假装持平。
    let direction = c.change_percent.map(|v| match v.cmp(&Decimal::ZERO) {
        std::cmp::Ordering::Greater => "is-up",
        std::cmp::Ordering::Less => "is-down",
        std::cmp::Ordering::Equal => "is-flat",
    });
    view! {
        <header class="company-header">
            <div class="ch-identity">
                <h2 class="ch-name">{name}</h2>
                {show_ticker.then(|| view! { <span class="ch-ticker">{c.ticker.clone()}</span> })}
            </div>
            <div class="ch-quote">
                {c.price.map(|p| view! {
                    <span class="ch-price">
                        {p.normalize().to_string()}
                        {(!currency.is_empty()).then(|| view! {
                            <small>{currency.clone()}</small>
                        })}
                    </span>
                })}
                {c.change_percent.map(|v| view! {
                    <span class=move || format!("ch-change {}", direction.unwrap_or("is-flat"))>
                        {format::signed_percent(v)}
                    </span>
                })}
                {c.market_cap.map(|m| view! {
                    <span class="ch-cap">"市值 " {format::compact_amount(m)}</span>
                })}
            </div>
        </header>
    }
}

/// 估值区间——刻度轴 + 逐法明细 + 关键假设。
///
/// 这里是「让每个判断都有证据」最该兑现的地方：只报三个数字，用户无从判断该不该信。
/// 轴让"现价落在带内还是带外"一眼可见（带外 = 贵/便宜），逐法明细摊开每条方法各自的
/// 结论——方法之间分歧大本身就是重要信息，被平均成一个数就看不见了。
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

    let axis = match (v.bear, v.bull, v.current_price) {
        (Some(bear), Some(bull), Some(price)) if bull > bear => {
            band_geometry(bear, bull, price).map(|geo| (geo, decimal_text(Some(price))))
        }
        _ => None,
    };
    let details = v.method_detail.clone();
    let assumptions = v.key_assumptions.clone();

    view! {
        <div class="valuation-block">
            <div class="valuation-head">
                <span>"估值区间"</span>
                <em>{v.method.clone()}</em>
            </div>

            <div class="val-scale">
                {axis.clone().map(|((left, width, price_x), price_text)| view! {
                    <div class="val-track">
                        <span
                            class="val-track-fill"
                            style=move || format!("left:{left}%;width:{width}%")
                        ></span>
                        <span
                            class="val-price-marker"
                            style=move || format!("left:{price_x}%")
                        >
                            <span class="val-price-dot"></span>
                            <span class="val-price-tag">"现价 " {price_text}</span>
                        </span>
                    </div>
                })}
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
            </div>

            {v.upside.clone().map(|u| {
                // 负空间 = 现价高于基准 = 贵。语义色只标方向，不替用户下判断。
                let cheap = !u.trim_start().starts_with('-');
                view! {
                    <p class="val-upside" class:is-cheap=cheap>
                        "相对现价 " <strong>{u}</strong>
                    </p>
                }
            })}

            {(!details.is_empty()).then(|| view! {
                <details class="val-methods">
                    <summary>
                        {format!("逐法明细（{} 条）", details.len())}
                    </summary>
                    <table class="val-method-table">
                        <thead>
                            <tr>
                                <th scope="col">"方法"</th>
                                <th scope="col">"熊"</th>
                                <th scope="col">"基准"</th>
                                <th scope="col">"牛"</th>
                            </tr>
                        </thead>
                        <tbody>
                            {details.into_iter().map(|m| view! {
                                <tr>
                                    <th scope="row">{m.name}</th>
                                    <td>{decimal_text(Some(m.bear))}</td>
                                    <td>{decimal_text(Some(m.base))}</td>
                                    <td>{decimal_text(Some(m.bull))}</td>
                                </tr>
                            }).collect_view()}
                        </tbody>
                    </table>
                    {(!assumptions.is_empty()).then(|| view! {
                        <ul class="val-assumptions">
                            {assumptions.into_iter()
                                .map(|a| view! { <li>{a}</li> })
                                .collect_view()}
                        </ul>
                    })}
                </details>
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
                aria-label="事实覆盖"
            >
                <span class="completeness-fill" style=move || format!("width:{completeness}%")></span>
            </div>
            <span class="completeness-label">{completeness_phrase(completeness)}</span>
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

/// 公司公告（SEC filings）——**一手证据**，与二手的网页证据卡分开呈现。
///
/// 此前后端取了公告、`connected_sources` 里也标着"最新公告"，前端却从不渲染：用户看得到
/// "有这个数据源"，却看不到是哪几份、哪一天、去哪读原文。一手披露是这个产品里可信度最高
/// 的一层证据，不该只当作一个统计数字。
#[component]
pub(crate) fn FilingCards(filings: Vec<echo_contracts::FilingView>) -> impl IntoView {
    if filings.is_empty() {
        return ().into_view();
    }
    view! {
        <div class="filing-cards">
            <p class="filing-cards-title">
                "公司公告 · " {filings.len()} " 份（一手披露）"
            </p>
            <ul class="filing-list">
                {filings.into_iter().map(|f| {
                    let date = f.filed_date.clone().unwrap_or_else(|| "未核到日期".into());
                    view! {
                        <li>
                            <a
                                class="filing-item"
                                href=f.source_url
                                target="_blank"
                                rel="noopener noreferrer"
                            >
                                <span class="filing-form">{f.form}</span>
                                <span class="filing-date">{date}</span>
                            </a>
                        </li>
                    }
                }).collect_view()}
            </ul>
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
        parts.push(completeness_phrase(value).to_string());
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

/// 流式进行中只保留一行来自服务端的真实阶段文字，不再额外叠加框、进度或步骤胶囊。
#[component]
fn StreamingCard(turn: Turn) -> impl IntoView {
    // 阶段与正文拆成两个 memo：delta 到达时只更新答案，不重挂思考文字。
    let stage = create_memo(move |_| {
        turn.status.with(|status| match status {
            TurnStatus::Streaming { stage, .. } => stage.clone(),
            _ => None,
        })
    });
    let delta_text = create_memo(move |_| {
        turn.status.with(|status| match status {
            TurnStatus::Streaming { delta_text, .. } => delta_text.clone(),
            _ => String::new(),
        })
    });
    view! {
        <div class="streaming-response">
            <p
                class="thinking-line"
                aria-live="polite"
                aria-label=move || {
                    let current = stage.get();
                    stage_activity(current.as_ref().map(|item| item.name))
                }
            >
                <strong class="thinking-shimmer">"正在思考"</strong>
            </p>
            {move || {
                let text = delta_text.get();
                (!text.is_empty()).then(|| {
                    let html = crate::markdown::render(&text);
                    view! { <div class="answer-text is-streaming" inner_html=html></div> }
                })
            }}
        </div>
    }
}

#[component]
fn ProcessedStatus(seconds: Option<u64>) -> impl IntoView {
    view! {
        <div class="processed-status">
            <strong>"已处理"</strong>
            {seconds.map(|value| view! { <span>{format!("{value}s")}</span> })}
        </div>
        <div class="processed-divider" aria-hidden="true"></div>
    }
}

#[component]
fn TurnBody(turn: Turn, on_retry: Callback<()>) -> impl IntoView {
    // Memo 只在 Streaming → 终态时通知父视图。流式期间 status 虽持续写入 delta，
    // 这里的布尔值没有变化，因此 StreamingCard 不会被卸载重建。
    let streaming = create_memo(move |_| turn.status.with(TurnStatus::is_streaming));
    view! {
        {move || {
            if streaming.get() {
                view! { <StreamingCard turn=turn /> }.into_view()
            } else {
                let seconds = turn.processed_seconds.get();
                match turn.status.get() {
                    TurnStatus::Done(response) => view! {
                        <div class="processed-response">
                            <ProcessedStatus seconds=seconds />
                            <DoneCard
                                res=*response
                                question=turn.question.get_value()
                                on_regenerate=on_retry
                            />
                        </div>
                    }.into_view(),
                    TurnStatus::CompareDone(response) => view! {
                        <div class="processed-response">
                            <ProcessedStatus seconds=seconds />
                            <CompareCard res=*response on_regenerate=on_retry />
                        </div>
                    }.into_view(),
                    TurnStatus::Archived(archived) => view! {
                        <HistoryCard
                            turn=*archived
                            question=turn.question.get_value()
                            ticker=turn.ticker.get_untracked()
                            on_regenerate=on_retry
                        />
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
                    TurnStatus::Streaming { .. } => unreachable!(),
                }
            }
        }}
    }
}

/// 答案上的动作条：复制原文、导出 Markdown、生成深度报告、重新生成。
///
/// 导出原来只挂在深度报告卡上；报告入口收掉后导出下沉到每一条答案——研究结论要能带走，
/// 这个能力不该跟着一个按钮一起消失。深度报告则以动作的形式回到这里：它走的是
/// `/api/report/generate`（报告专属提示词 + 固定结构 + 同一份护栏），产出的是一份可归档的
/// Markdown，而不是对话里的又一段答案，所以直接落成文件而不占用对话流。
///
/// `question` 缺省时不出深度报告按钮——对比轮就是这种情况：报告服务是单主体口径，
/// 硬给对比轮配一个报告按钮，点下去只会拿到其中一条腿的报告，比没有按钮更误导。
#[component]
fn AnswerActions(
    text: Option<String>,
    #[prop(into)] ticker: String,
    question: Option<String>,
    session_id: Option<String>,
    on_regenerate: Callback<()>,
) -> impl IntoView {
    let (copied, set_copied) = create_signal(false);
    let export = text.clone();
    let report_ticker = ticker.clone();
    let report_question = question.clone();
    let report = create_action(move |(): &()| {
        let req = AskRequest {
            question: report_question.clone().unwrap_or_default(),
            ticker: report_ticker.clone(),
            session_id: session_id.clone(),
            ..Default::default()
        };
        let filename = if report_ticker.trim().is_empty() {
            "echo-深度报告.md".to_string()
        } else {
            format!("{}-深度报告.md", report_ticker.trim())
        };
        async move {
            let response =
                api::post::<_, ReportGenerateResponse>("/api/report/generate", &req).await?;
            api::download_text_file(&filename, "text/markdown;charset=utf-8", &response.markdown);
            Ok::<(), String>(())
        }
    });
    let offers_report = question.is_some();
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
            {offers_report.then(|| view! {
                <button
                    class="answer-action"
                    title="用同一个问题生成一份结构化深度报告并下载"
                    disabled=move || report.pending().get()
                    on:click=move |_| report.dispatch(())
                >
                    <Icon name="file" />
                    {move || if report.pending().get() { "生成报告中…" } else { "深度报告" }}
                </button>
            })}
            <button
                class="answer-action"
                title="用同一个问题重新研究一次"
                on:click=move |_| on_regenerate.call(())
            >
                <Icon name="refresh" />"重新生成"
            </button>
            // 报告失败要说出来。静默失败会让用户以为"点了没反应"，然后一直点。
            {move || report.value().get().and_then(Result::err).map(|error| view! {
                <span class="answer-action-error" role="alert">{error}</span>
            })}
        </div>
    }
}

/// 干净完成后的答案卡——答案文本优先，估值/完备度/来源/护栏全部收进折叠的证据面板。
#[component]
fn DoneCard(
    res: AskResponse,
    #[prop(into)] question: String,
    on_regenerate: Callback<()>,
) -> impl IntoView {
    let answer_text = res.answer.clone();
    view! {
        <div class="answer-card">
            {res.company.clone().map(|c| view! { <CompanyHeader c=c /> })}
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

            <AnswerActions
                text=answer_text
                ticker=res.ticker.clone()
                question=Some(question)
                session_id=res.session_id.clone()
                on_regenerate=on_regenerate
            />

            <FilingCards filings=res.filings.clone() />

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
///
/// 动作条与实时作答一视同仁。此前重开一条历史会话，复制、导出、深度报告、重新生成会
/// **整条消失**——研究结论一旦落库反而带不走了，这跟"研究历史"的存在意义正好相反。
/// `session_id` 传 `None`：深度报告按这一轮的问题与主体现取现做，不假装能复原当时的上下文。
#[component]
fn HistoryCard(
    turn: ArchivedTurn,
    #[prop(into)] question: String,
    #[prop(into)] ticker: String,
    on_regenerate: Callback<()>,
) -> impl IntoView {
    let answer_text = turn.answer.clone();
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

            <AnswerActions
                text=answer_text
                ticker=ticker
                question=Some(question)
                session_id=None
                on_regenerate=on_regenerate
            />

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
                question=None
                session_id=None
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
    let label = if cancelled {
        "已取消"
    } else {
        "请求未成功"
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
        // 窄屏展开时抽屉是覆盖在研究台之上的。没有遮罩，用户只能回到抽屉里那个 `<`
        // 才关得掉——覆盖式抽屉点外面就该收起来。宽屏用 CSS 关掉这层（那里抽屉是
        // 常驻栏，不覆盖任何东西）。
        {move || (!collapsed.get()).then(|| view! {
            <div
                class="sidebar-scrim"
                aria-hidden="true"
                on:click=move |_| set_collapsed.set(true)
            ></div>
        })}
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
                        crate::workspace::empty_view(
                            "还没有研究记录。",
                            "在下方输入框问第一个问题，研究就会存到这里。",
                        )
                    }
                    Some(Ok(data)) => {
                        let active_id = active_id.clone();
                        // 按天分组：几十条平铺时"这是今天问的还是上周问的"完全读不出来，
                        // 时间戳挤在每一行的副标题里也扫不动。分组把时间提到组标题上，
                        // 组内每行就只剩问题本身与代码。
                        group_sessions_by_day(data.sessions).into_iter().map(|(day, items)| {
                            let active_id = active_id.clone();
                            view! {
                                <section class="session-group">
                                    <h3 class="session-group-title">{day}</h3>
                                    {items.into_iter().map(|item| {
                                        let is_active = active_id.as_deref() == Some(item.id.as_str());
                                        let go_id = item.id.clone();
                                        let del_id = item.id.clone();
                                        let ticker = item.ticker.clone().unwrap_or_default();
                                        let time = format::clock(&item.updated_at);
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
                                                        <span>{time.clone()}</span>
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
                                    }).collect_view()}
                                </section>
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
    let pending = move || {
        thread
            .get()
            .iter()
            .any(|turn| turn.status.get().is_streaming())
    };
    let on_persisted = Callback::new(move |_| sessions.refetch());
    let on_activity = Callback::new(move |_| activity.update(|value| *value += 1));

    // 新消息与一轮终态到达时让阅读位置跟到最新内容；逐字增量不滚动，避免整页持续上移。
    #[cfg(target_arch = "wasm32")]
    {
        let scroll_target = conversation_ref;
        create_effect(move |previous: Option<usize>| {
            let turn_count = thread.get().len();
            let _ = activity.get();
            // 新增一轮时强制贴底（用户刚提交，必须看到自己的消息）。
            let force = previous.is_some_and(|count| turn_count > count);
            // 节点在 effect 内取出——此刻组件一定还活着。rAF 回调只持有这个 DOM 引用，
            // 不再回头读 NodeRef：回调可能在本组件卸载之后才执行（提交完立刻切到设置页
            // 就会这样），那时信号已随作用域释放，读它会 panic「already been disposed」。
            if let Some(node) = scroll_target.get_untracked() {
                request_animation_frame(move || follow_bottom(&node, force));
            }
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
    let followups = [
        "现在最值得关注的三个信号是什么？",
        "什么情况会证伪当前判断？",
        "用更简洁的结论总结一下。",
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
                                <p class="hero-kicker"><span aria-hidden="true"></span>"ECHO INTELLIGENCE"</p>
                                <h1>
                                    <span class="line-1">"让复杂信息，"</span>
                                    <span class="line-2">"收敛成清晰判断。"</span>
                                </h1>
                                <p class="hero-copy">"从公司、估值、风险或证伪开始提问。ECHO 会连接可核验的数据与来源，给出有边界的研究答案。"</p>
                                <div class="hero-trust-row" aria-label="研究能力">
                                    <span>"实时行情"</span><i></i><span>"原始披露"</span><i></i><span>"数字护栏"</span>
                                </div>
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
                                                    // 卡片写着"开始研究"就必须真的开始研究。此前只把问题填进
                                                    // 输入框，用户还得再点一次发送——标签承诺与行为不符。
                                                    on:click=move |_| {
                                                        set_subject.set(ticker.to_string());
                                                        set_question.set(prompt.to_string());
                                                        submit();
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
                                        // 对话保持最少结构：一句问题，不再叠加作者、代码标签或气泡框。
                                        <div class="message user">
                                            <p class="bubble-text">{turn.question.get_value()}</p>
                                        </div>
                                        // 助手也不再套身份头和卡片；流式时就是一行真实思考文字。
                                        <div class="message assistant">
                                            <div class="assistant-message">
                                                <TurnBody turn=turn on_retry=on_retry />
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
            // 里面只有一个输入框和一个按钮：没有研究对象选择器、没有股票代码标签、
            // 没有第二条提交通道、没有快捷键说明和免责小字。主体全程由服务端从问题文本
            // 识别，是对话的内部状态，不是用户要先填对的一个字段。
            <div class="composer">
                {move || (has_thread() && !subject.get().is_empty()).then(|| view! {
                    <div class="composer-suggestions" aria-label="快捷追问">
                        {followups.into_iter().map(|prompt| view! {
                            <button
                                disabled=pending
                                on:click=move |_| {
                                    set_question.set(prompt.to_string());
                                    submit();
                                }
                            >{prompt}</button>
                        }).collect_view()}
                    </div>
                })}
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
                        // 提示语里不出现股票代码。研究是以对话为中心的：主体由服务端从
                        // 问题文本识别并在对话里承接，编辑器不该把内部识别结果当标签挂出来，
                        // 那会让人以为必须先"选对主体"才能提问。
                        placeholder=move || if has_thread() {
                            "继续追问…"
                        } else {
                            "输入公司、代码或你想研究的问题"
                        }
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
