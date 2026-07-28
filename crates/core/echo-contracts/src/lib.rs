//! Echo Research 的端到端契约单一事实源。
//!
//! axum 边界与 Leptos/WASM 直接依赖这些类型，避免服务端与前端各手抄一套 JSON 结构后静默漂移。
//! 金融数字保留 [`rust_decimal::Decimal`]，JSON 往返不经过二进制浮点。

pub use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HealthResponse {
    pub status: String,
    pub service: String,
    pub stack: String,
}

impl HealthResponse {
    #[must_use]
    pub fn ok() -> Self {
        Self {
            status: "ok".into(),
            service: "echo-api".into(),
            stack: "rust".into(),
        }
    }
}

/// 研究入口请求。除 `question` / `ticker` 外的字段均为已核实事实；缺失就是缺失，不补零。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskRequest {
    pub question: String,
    /// 研究主体代码。可省略/留空——服务端会从问题文本走公司解析链识别主体；
    /// 识别失败时诚实报错，不猜。
    #[serde(default)]
    pub ticker: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_zh: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote_currency: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reporting_currency: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pe: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub market_cap: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_percent: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eps: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eps_annualized: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub net_margin: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gross_margin: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revenue: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revenue_growth: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub net_income: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shares_outstanding: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub free_cash_flow: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub net_cash: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft_answer: Option<String>,
    /// 已有会话 id——续问同一研究会话（同 ticker 追问）时带上，落库归位同一行、
    /// 历史只用于代词/实体承接，不作为本轮数字核对依据。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

impl AskRequest {
    #[must_use]
    pub fn minimal(question: impl Into<String>, ticker: impl Into<String>) -> Self {
        Self {
            question: question.into(),
            ticker: ticker.into(),
            ..Self::default()
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteView {
    pub intent: String,
    pub depth: String,
    pub confidence: f64,
    pub multi_part: bool,
    pub answer_style: String,
    pub plan: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuardView {
    pub total: usize,
    pub pass: usize,
    pub soft: usize,
    pub hard: usize,
    pub has_hard_fail: bool,
    pub soft_note: String,
}

/// 定性引用护栏视图——与数字护栏（`GuardView`）互补，核的是"定性论断有没有标注真实来源号"。
/// 只在本轮有网页证据时出现（`evidence_count>0`）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CitationGuardView {
    pub evidence_count: usize,
    pub cited_count: usize,
    /// 引用了不存在的来源号的个数（虚构引用，硬信号）。
    pub out_of_range: usize,
    /// 有证据却零引用（定性论断裸奔）。
    pub ungrounded: bool,
    /// 有虚构来源号——展示层据此标红。
    pub has_hard_fail: bool,
    /// 低调中文提示（无问题时为空串）。
    pub note: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AssetStageView {
    Profitable,
    LossGrowth,
    Loss,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MethodBandView {
    pub name: String,
    pub bear: Decimal,
    pub base: Decimal,
    pub bull: Decimal,
}

/// 研究主体的行情抬头——答案上方"这是哪家公司、现在多少钱"的一行事实。
/// 每个字段各自可缺：缺什么就不画什么，绝不用 0 或旧值占位。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompanyHeaderView {
    pub ticker: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_percent: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub market_cap: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValuationView {
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bear: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bull: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upside: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub downside: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_price: Option<Decimal>,
    pub methods: Vec<String>,
    pub method_detail: Vec<MethodBandView>,
    pub key_assumptions: Vec<String>,
    pub sensitivity: Vec<String>,
    #[serde(default)]
    pub stage_aware: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<AssetStageView>,
    #[serde(default)]
    pub data_suspect: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cannot_value_reason: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnswerSource {
    Draft,
    Generated,
    Unavailable,
}

impl AnswerSource {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Generated => "generated",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EarningsCalendarView {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quarter: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eps_estimate: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revenue_estimate: Option<Decimal>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilingView {
    pub form: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filed_date: Option<String>,
    pub source_url: String,
}

/// 一条网页证据来源卡（Tavily 检索）——供 Web 渲染可点击来源。二手来源，仅定性支撑。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceView {
    pub title: String,
    pub url: String,
    pub snippet: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_domain: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskResponse {
    pub ticker: String,
    /// 研究主体的行情抬头（名称/现价/涨跌/市值）。缺行情即 `None`——抬头宁可不画，
    /// 也不用占位数字冒充"已核到"。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub company: Option<CompanyHeaderView>,
    pub route: RouteView,
    pub data_completeness: u8,
    pub connected_sources: Vec<String>,
    pub valuation: ValuationView,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    pub answer_source: AnswerSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fact_guard: Option<GuardView>,
    /// 定性引用护栏（仅本轮有网页证据时出现）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub citation_guard: Option<CitationGuardView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub earnings: Option<EarningsCalendarView>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filings: Vec<FilingView>,
    /// 本轮拉到的网页证据来源卡（定性意图才有；数字驱动意图与失败降级时为空）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<EvidenceView>,
    /// 本轮落库归属的会话 id——落库失败且是新会话时为 `None`；Web 拿到后带回下一轮
    /// `AskRequest.session_id` 即可续接同一研究会话。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

/// 类型化研究 SSE 事件。`type` 字段与 Axum `event:` 名对齐。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum ResearchStreamEvent {
    Meta(Box<ResearchStreamMeta>),
    Stage(ResearchStreamStage),
    Delta(ResearchStreamDelta),
    Guard(ResearchStreamGuard),
    Final(Box<ResearchStreamFinal>),
    Compare(Box<ResearchStreamCompare>),
    Error(ResearchStreamError),
}

impl ResearchStreamEvent {
    /// SSE `event:` 名（与 serde tag 一致）。
    #[must_use]
    pub fn event_name(&self) -> &'static str {
        match self {
            Self::Meta(_) => "meta",
            Self::Stage(_) => "stage",
            Self::Delta(_) => "delta",
            Self::Guard(_) => "guard",
            Self::Final(_) => "final",
            Self::Compare(_) => "compare",
            Self::Error(_) => "error",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResearchStreamMeta {
    pub ticker: String,
    /// 行情抬头随 meta 一起到达——取数早于作答，抬头能在第一个 token 之前就位，
    /// 用户在等答案时先看到"研究的是哪家、现在多少钱"。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub company: Option<CompanyHeaderView>,
    pub route: RouteView,
    pub data_completeness: u8,
    pub connected_sources: Vec<String>,
    pub valuation: ValuationView,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub earnings: Option<EarningsCalendarView>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResearchStreamStageName {
    Routing,
    Resolving,
    MarketFinancials,
    Evidence,
    Valuation,
    FactCheck,
    /// 旧客户端兼容；新服务端改发与 `route.plan` 同名的精确阶段。
    Assembling,
    Generating,
    Verifying,
    Persisting,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResearchStreamStage {
    pub name: ResearchStreamStageName,
    /// 当前步骤在 `route.plan` 中的 1-based 序号。
    #[serde(default)]
    pub index: usize,
    /// 本轮 `route.plan` 的步骤总数。
    #[serde(default)]
    pub total: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResearchStreamDelta {
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResearchStreamGuard {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fact_guard: Option<GuardView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub citation_guard: Option<CitationGuardView>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResearchStreamFinal {
    pub response: AskResponse,
    pub persisted: bool,
}

/// 对话内双主体对比完成事件——两腿独立取数/独立护栏的结果一次性到达
/// （对比无逐字流，模型作答是单次调用）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResearchStreamCompare {
    pub response: CompareResponse,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResearchStreamError {
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UserRole {
    Owner,
    Member,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct PublicUser {
    pub id: String,
    pub username: String,
    pub display_name: Option<String>,
    pub role: UserRole,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthLoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct AuthRegisterRequest {
    pub invite: String,
    pub username: String,
    pub password: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthInviteRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthUserResponse {
    pub user: PublicUser,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct AccountSubscription {
    pub plan_id: String,
    pub plan_name: String,
    pub tier: String,
    pub status: String,
    pub current_period_end: String,
    pub max_daily_calls: i32,
    pub features: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountResponse {
    pub user: PublicUser,
    pub subscription: Option<AccountSubscription>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct AuthMeResponse {
    pub user: Option<PublicUser>,
    pub multi_user: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct AuthLogoutResponse {
    pub logged_out: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthInviteResponse {
    pub code: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorResponse {
    pub message: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompanySearchQuery {
    #[serde(default)]
    pub q: String,
    #[serde(default)]
    pub limit: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CompanySearchItem {
    pub ticker: String,
    pub name_zh: String,
    pub name_en: Option<String>,
    pub sector: Option<String>,
    pub industry: Option<String>,
    pub has_portrait: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompanySearchResponse {
    pub companies: Vec<CompanySearchItem>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompanyResolveQuery {
    #[serde(default)]
    pub q: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CompanyResolveItem {
    pub ticker: String,
    pub name_zh: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_en: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub industry: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompanyResolveResponse {
    pub company: Option<CompanyResolveItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompanyVerifyQuery {
    #[serde(default)]
    pub ticker: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompanyVerifySuggestion {
    pub ticker: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompanyVerifyResponse {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestions: Option<Vec<CompanyVerifySuggestion>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WatchEntry {
    pub ticker: String,
    pub company_name: Option<String>,
    pub mode: String,
    pub created_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WatchListResponse {
    pub entries: Vec<WatchEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WatchMutationRequest {
    pub ticker: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub company_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MutationResponse {
    pub changed: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WatchRule {
    pub id: i64,
    pub ticker: String,
    pub kind: String,
    pub threshold: Decimal,
    pub metric: Option<String>,
    pub label: Option<String>,
    pub active: bool,
    pub created_at: String,
    pub last_triggered_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WatchRulesListResponse {
    pub rules: Vec<WatchRule>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WatchRuleCreateRequest {
    pub ticker: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metric: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WatchRuleDeleteRequest {
    pub id: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DeskTicker {
    pub ticker: String,
    pub company_name: Option<String>,
    pub price: Option<Decimal>,
    pub change_percent: Option<Decimal>,
    pub rules: Vec<WatchRule>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DeskResponse {
    pub tickers: Vec<DeskTicker>,
    pub recent_triggers: Vec<Notification>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PortfolioPosition {
    pub ticker: String,
    pub company_name: String,
    pub shares: Option<Decimal>,
    pub avg_cost: Option<Decimal>,
    pub stop_loss: Option<Decimal>,
    pub take_profit: Option<Decimal>,
    pub note: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortfolioListResponse {
    pub positions: Vec<PortfolioPosition>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PortfolioUpsertRequest {
    pub ticker: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub company_name: Option<String>,
    pub shares: Decimal,
    pub avg_cost: Decimal,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_loss: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub take_profit: Option<Decimal>,
    #[serde(default)]
    pub note: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TickerQuery {
    pub ticker: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UserPreferences {
    pub onboarding_completed: bool,
    pub notify_digest: bool,
    pub notify_positions: bool,
    pub notify_falsify: bool,
    pub notify_review: bool,
    pub notify_earnings: bool,
    pub quiet_hours_start: Option<String>,
    pub quiet_hours_end: Option<String>,
}

impl Default for UserPreferences {
    fn default() -> Self {
        Self {
            onboarding_completed: false,
            notify_digest: true,
            notify_positions: true,
            notify_falsify: true,
            notify_review: true,
            notify_earnings: true,
            quiet_hours_start: None,
            quiet_hours_end: None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PreferencesUpdateRequest {
    #[serde(default)]
    pub onboarding_completed: Option<bool>,
    #[serde(default)]
    pub notify_digest: Option<bool>,
    #[serde(default)]
    pub notify_positions: Option<bool>,
    #[serde(default)]
    pub notify_falsify: Option<bool>,
    #[serde(default)]
    pub notify_review: Option<bool>,
    #[serde(default)]
    pub notify_earnings: Option<bool>,
    #[serde(default)]
    pub quiet_hours_start: Option<String>,
    #[serde(default)]
    pub quiet_hours_end: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreferencesResponse {
    pub preferences: UserPreferences,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Notification {
    pub id: i64,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub ticker: Option<String>,
    pub payload: Option<serde_json::Value>,
    pub created_at: String,
    pub read_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationsListResponse {
    pub notifications: Vec<Notification>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnreadResponse {
    pub unread: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationReadRequest {
    #[serde(default)]
    pub id: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangedCountResponse {
    pub changed: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListQuery {
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub ticker: Option<String>,
}

/// 公司档案摘要（列表视图）——对标 honeclaw 的 Markdown 长期记忆，但每条数字可溯源、
/// 过护栏，不是自由文本堆砌。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompanyProfileSummary {
    pub ticker: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub company_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub research_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<String>,
    pub turn_count: i32,
    pub updated_at: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompanyProfileDetail {
    pub ticker: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub company_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thesis: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub research_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<String>,
    #[serde(default)]
    pub bull: Vec<String>,
    #[serde(default)]
    pub bear: Vec<String>,
    #[serde(default)]
    pub monitors: Vec<String>,
    #[serde(default)]
    pub falsifiers: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valuation_method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valuation_bear: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valuation_base: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valuation_bull: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valuation_current_price: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_md: Option<String>,
    pub turn_count: i32,
    pub created_at: String,
    pub updated_at: String,
}

/// 部分更新请求——省略字段表示"本次不改"，与 `CompanyProfileUpsert`（echo-db）同一语义。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompanyProfileUpsertRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub company_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thesis: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub research_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bull: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bear: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monitors: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub falsifiers: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valuation_method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valuation_bear: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valuation_base: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valuation_bull: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valuation_current_price: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_md: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompanyProfilesListResponse {
    pub profiles: Vec<CompanyProfileSummary>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompanyProfileResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<CompanyProfileDetail>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ResearchSessionSummary {
    pub id: String,
    pub ticker: Option<String>,
    pub title: String,
    pub question: String,
    pub conversation_id: String,
    pub status: String,
    pub rating: Option<String>,
    pub confidence: Option<String>,
    pub turn_count: i32,
    pub company_name: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResearchSessionsResponse {
    pub sessions: Vec<ResearchSessionSummary>,
    pub count: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ResearchSessionDetail {
    pub id: String,
    pub ticker: Option<String>,
    pub title: String,
    pub question: String,
    pub conversation_id: String,
    pub status: String,
    pub report_markdown: Option<String>,
    pub rating: Option<String>,
    pub confidence: Option<String>,
    pub decision_panel: Option<serde_json::Value>,
    pub full_research: Option<String>,
    pub data_sources: Option<serde_json::Value>,
    pub thread: Option<serde_json::Value>,
    pub turn_count: i32,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResearchSessionResponse {
    pub session: Option<ResearchSessionDetail>,
}

/// 双主体对比研究入口——两个 ticker 各自独立取数，绝无"问苹果答腾讯"的合并事实面。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompareRequest {
    pub question: String,
    pub primary_ticker: String,
    pub peer_ticker: String,
}

/// 对比研究里单腿的结构化事实——与单公司 `AskResponse` 同源字段，独立不共享。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompareLegView {
    pub ticker: String,
    pub data_completeness: u8,
    pub connected_sources: Vec<String>,
    pub valuation: ValuationView,
    /// 本腿自己的网页证据；A/B 两腿分别返回，展示与 prompt 都不合并。
    #[serde(default)]
    pub sources: Vec<EvidenceView>,
    /// 该腿自己的护栏结果——只用本腿的 `FactsRegistry` 核对整段作答，两腿互不合并、
    /// 互不借用对方的事实登记表（"分别验证"）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fact_guard: Option<GuardView>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompareResponse {
    pub route: RouteView,
    pub primary: CompareLegView,
    pub peer: CompareLegView,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    pub answer_source: AnswerSource,
}

/// 深度报告的生成方式——模型生成，或模型不可用/输出过短时的本地确定性兜底。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportMode {
    Model,
    Local,
}

impl ReportMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Model => "report_model",
            Self::Local => "report_local",
        }
    }
}

/// 深度报告入口响应——复用 `AskRequest` 作为请求体（同一套事实覆盖字段与 `session_id`
/// 续接语义），报告只引用 `FactsRegistry` 内已核数字（同一份护栏，见 `fact_guard`）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportGenerateResponse {
    pub ticker: String,
    pub route: RouteView,
    pub mode: ReportMode,
    pub markdown: String,
    pub valuation: ValuationView,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fact_guard: Option<GuardView>,
    /// 定性引用护栏（仅本轮有网页证据时出现）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub citation_guard: Option<CitationGuardView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub earnings: Option<EarningsCalendarView>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filings: Vec<FilingView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;
    use std::str::FromStr;

    #[test]
    fn minimal_request_omits_unknown_financials() {
        let json =
            serde_json::to_value(AskRequest::minimal("腾讯估值？", "0700.HK")).expect("serialize");
        assert_eq!(json["question"], "腾讯估值？");
        assert_eq!(json["ticker"], "0700.HK");
        assert!(json.get("price").is_none());
    }

    #[test]
    fn decimal_request_round_trips_exactly() {
        let mut request = AskRequest::minimal("AAPL 利润质量", "AAPL");
        request.price = Some(Decimal::from_str("201.123456789").expect("decimal"));
        let encoded = serde_json::to_string(&request).expect("serialize");
        let decoded: AskRequest = serde_json::from_str(&encoded).expect("deserialize");
        assert_eq!(decoded, request);
    }

    #[test]
    fn unknown_request_field_is_rejected() {
        let error =
            serde_json::from_str::<AskRequest>(r#"{"question":"q","ticker":"AAPL","made_up":1}"#)
                .expect_err("unknown field must fail");
        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn answer_source_wire_values_are_stable() {
        assert_eq!(
            serde_json::to_string(&AnswerSource::Unavailable).expect("serialize"),
            r#""unavailable""#
        );
    }

    #[test]
    fn research_stream_event_tags_are_stable() {
        let meta = ResearchStreamEvent::Meta(Box::new(ResearchStreamMeta {
            ticker: "AAPL".into(),
            company: None,
            route: RouteView {
                intent: "valuation".into(),
                depth: "brief".into(),
                confidence: 0.9,
                multi_part: false,
                answer_style: "direct".into(),
                plan: vec!["routing".into()],
            },
            data_completeness: 20,
            connected_sources: vec!["实时行情".into()],
            valuation: ValuationView {
                method: "unavailable".into(),
                bear: None,
                base: None,
                bull: None,
                upside: None,
                downside: None,
                current_price: None,
                methods: vec![],
                method_detail: vec![],
                key_assumptions: vec![],
                sensitivity: vec![],
                stage_aware: false,
                stage: None,
                data_suspect: false,
                cannot_value_reason: None,
            },
            earnings: None,
        }));
        let json = serde_json::to_value(&meta).expect("serialize");
        assert_eq!(json["type"], "meta");
        assert_eq!(json["data"]["ticker"], "AAPL");
        assert_eq!(meta.event_name(), "meta");

        let delta = ResearchStreamEvent::Delta(ResearchStreamDelta {
            text: "现价".into(),
        });
        let encoded = serde_json::to_string(&delta).expect("serialize");
        let decoded: ResearchStreamEvent = serde_json::from_str(&encoded).expect("roundtrip");
        assert_eq!(decoded, delta);

        let err = serde_json::from_str::<ResearchStreamEvent>(
            r#"{"type":"delta","data":{"text":"x","extra":1}}"#,
        )
        .expect_err("unknown field");
        assert!(err.to_string().contains("unknown field"));
    }
}
