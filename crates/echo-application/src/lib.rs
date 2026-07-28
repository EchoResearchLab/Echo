//! 用例编排——研究链路（取数 → 估值 → 作答 → 护栏 → 落库）。
//!
//! HTTP 边界只做鉴权与序列化；非流式研究由 [`research::ResearchService`] 统一编排。
//!
//! 关键修复（对应本次诊断的研究质量根因）：`ResearchRequest` 显式携带**被解析出的单一公司**，
//! 决策面板与作答上下文都以它为唯一事实主体——跨公司的历史/记忆一律只作代词承接，
//! 不得携带别家的财务数字（"问苹果答腾讯"的架构级封堵）。

use echo_domain::{
    Company, Filing, Financials, MarketSnapshot, PeerAnchor, Valuation, display_valuation,
};

pub mod answer_prompt;
pub mod auth;
pub mod company_resolve;
pub mod from_db;
pub mod model_gateway;
pub mod report;
pub mod research;
pub mod research_memory;
pub mod research_orchestrator;
pub mod watch_rules;

pub use answer_prompt::{AnswerContext, build_system_prompt, build_user_prompt};
pub use auth::{AuthError, AuthService, Session, hash_password, verify_password};
pub use company_resolve::{
    CompanyResolvePorts, CompanyResolveService, DbCompanyHit, ExternalSymbolHit, ResolveResult,
    ResolveSource, ResolveSuggestion, ResolvedListing, VerifyResult, VerifyStatus,
};
pub use from_db::{market_snapshot_from_rows, resolved_company_from_rows};
pub use model_gateway::{
    AnswerKind, ModelAnswer, ModelAnswerOptions, ModelStreamEvent, ModelStreamStart,
    OwnedAuditContext, ProviderConfig, model_answer, model_answer_stream, parse_json_object,
};
pub use report::{ReportOutcome, ReportService};
pub use research::{
    CompareResearchFacts, FactRecovery, FactRecoveryRequest, GuardAuditRecord, GuardOutcome,
    LoadedFundamentals, PersistResearchSession, PriorTurn, ResearchFacts, ResearchOutcome,
    ResearchPorts, ResearchService,
};
pub use research_memory::{CompanyMemory, CompanyMemoryUpdate};
pub use research_orchestrator::{
    DEFAULT_MAX_RECOVERY_ROUNDS, RecoveryRound, RecoveryTrace, ResearchDegradation, ResearchGap,
    ResearchLoopConfig, ResearchOrchestrator, evaluate_gaps,
};
pub use watch_rules::{WatchRuleError, WatchRuleService};

/// 一轮研究请求——公司是必到字段（由公司解析闭环在边界处兑现），history 只用于代词承接。
#[derive(Clone, Debug)]
pub struct ResearchRequest {
    pub question: String,
    pub company: ResolvedCompany,
    pub compare_with: Option<ResolvedCompany>,
}

/// 已解析、已核实的研究主体。
#[derive(Clone, Debug)]
pub struct ResolvedCompany {
    pub ticker: String,
    pub name_zh: Option<String>,
    pub company: Company,
}

/// 决策面板的数据完备度——今天能核到的五个源里站住了几个（不是产品评分）。
#[derive(Clone, Debug)]
pub struct DecisionPanel {
    pub ticker: String,
    pub valuation: Valuation,
    pub connected_sources: Vec<&'static str>,
    pub data_completeness: u8,
}

/// 用已取到的数据算估值并拼出决策面板。**只吃当前公司的财务事实**，无跨公司泄漏面。
#[must_use]
pub fn build_panel(
    company: &ResolvedCompany,
    market: &MarketSnapshot,
    financials: &Financials,
    peer: Option<&PeerAnchor>,
    filings: &[Filing],
) -> DecisionPanel {
    let valuation = display_valuation(&company.company, market, financials, peer);

    let mut connected = Vec::new();
    if market.price.is_some() {
        connected.push("实时行情");
    }
    if financials.provider_ok {
        connected.push("财报口径");
    }
    if valuation.is_valued() {
        connected.push("估值区间");
    }
    if peer.is_some() {
        connected.push("同业对比");
    }
    if !filings.is_empty() {
        connected.push("最新公告");
    }
    let completeness = ((connected.len() as f64 / 5.0) * 100.0).round() as u8;

    DecisionPanel {
        ticker: company.ticker.clone(),
        valuation,
        connected_sources: connected,
        data_completeness: completeness,
    }
}
