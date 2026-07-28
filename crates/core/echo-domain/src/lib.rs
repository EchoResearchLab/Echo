//! Echo Research 领域内核——纯逻辑，不碰时钟、数据库、网络或环境变量。
//!
//! 领域模块清单：
//!   * [x] `valuation`   ← valuation.js（多方法估值 + 阶段感知 EV/Sales + 可信度护栏）
//!   * [x] `fact_guard`  ← factGuard.js（数字护栏：中文财经抽取 + 数量级/符号/币种/日期核对）
//!   * [x] `intent`      ← intentClassifier.js（意图路由 + 深度 + 阶段计划，中英双语一等公民）
//!   * [x] `derivations` ← research.ts 的 deriveAnnualEps / 比率衍生（TTM 桥接 + EPS 年化护栏）
//!
//! 领域核保持无副作用，可在服务端与浏览器端复用并独立测试。

pub mod company_aliases;
pub mod company_identity;
pub mod derivations;
pub mod fact_guard;
pub mod intent;
pub mod valuation;
pub mod watch_rules;

pub use company_aliases::{
    CompanyAlias, CompanyMention, HK_COMPANY_ALIASES, HK_US_LINKS, HkUsLink, HkUsLinkKind,
    US_COMPANY_ALIASES, adr_for_hk, company_identity_key, dual_listing_by_name,
    dual_listing_by_ticker, dual_listings, has_compare_cue, match_company_mentions, match_hk_alias,
    match_us_alias,
};
pub use company_identity::{
    common_non_tickers, extract_hk_ticker, extract_us_ticker_token, normalize_question_text,
};
pub use derivations::{AnnualEps, FilingRow, derive_annual_eps, pct_change, pct_of};
pub use fact_guard::{
    CitationReport, FactsRegistry, Position, RegistrySources, Verdict, VerifyReport,
    build_facts_registry, build_soft_note, merge_facts_registry, render_hard_fail_issues,
    verify_answer_citations, verify_answer_numbers,
};
pub use intent::{
    ResearchDepth, ResearchIntent, ResearchRoute, classify_research_intent,
    intent_wants_web_evidence, plan_research_stages, route_research_intent,
};
pub use valuation::{
    AssetStage, Company, EarningsCalendar, Evidence, Filing, Financials, HistoricalValuation,
    HkBuyback, InsiderActivity, MarketSnapshot, MethodBand, MultipleType, PeerAnchor, Valuation,
    classify_asset_stage, compute_valuation, display_valuation,
};
pub use watch_rules::RuleKind;
