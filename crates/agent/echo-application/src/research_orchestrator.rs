//! 缺口驱动的研究补救编排。
//!
//! [`ResearchService`](crate::research::ResearchService) 负责执行一次具体取数；本模块站在它外面，
//! 按当前意图评估事实是否足够、把缺口交给数据端换源，并在有进展时继续下一轮。循环有硬上限，
//! 同一缺口没有变化就提前停止，避免供应商故障时反复请求。

use crate::build_panel;
use crate::research::{
    FactRecovery, FactRecoveryRequest, ResearchFacts, ResearchPorts, ResearchService,
};
use echo_contracts::AskRequest;
use echo_domain::{
    AssetStage, Financials, MarketSnapshot, MultipleType, ResearchIntent, ResearchRoute,
    classify_asset_stage,
};
use serde::{Deserialize, Serialize};

/// 默认最多补救两轮：第一轮换一手/官方来源，第二轮补独立估值或同业锚点。
pub const DEFAULT_MAX_RECOVERY_ROUNDS: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResearchGap {
    MarketPrice,
    FinancialStatements,
    TtmEps,
    Revenue,
    SharesOutstanding,
    HistoricalValuation,
    PeerComparison,
    RecentFilings,
    WebEvidence,
    Valuation,
}

impl ResearchGap {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MarketPrice => "market_price",
            Self::FinancialStatements => "financial_statements",
            Self::TtmEps => "ttm_eps",
            Self::Revenue => "revenue",
            Self::SharesOutstanding => "shares_outstanding",
            Self::HistoricalValuation => "historical_valuation",
            Self::PeerComparison => "peer_comparison",
            Self::RecentFilings => "recent_filings",
            Self::WebEvidence => "web_evidence",
            Self::Valuation => "valuation",
        }
    }
}

/// 估值仍无法成立时的显式降级，不让模型把“没有目标价”误解成“可以自己估一个”。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResearchDegradation {
    #[default]
    None,
    /// 公司自身估值基数不足，但同业分位已核到：只允许做同业倍数对照。
    PeerComparisonOnly,
    /// 一手财务仍缺：只允许定性研究。
    QualitativeOnly,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryRound {
    pub round: usize,
    pub requested: Vec<ResearchGap>,
    pub remaining: Vec<ResearchGap>,
    pub made_progress: bool,
    pub sources: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryTrace {
    pub rounds: Vec<RecoveryRound>,
    pub final_gaps: Vec<ResearchGap>,
    pub degradation: ResearchDegradation,
}

#[derive(Clone, Copy, Debug)]
pub struct ResearchLoopConfig {
    pub max_rounds: usize,
}

impl Default for ResearchLoopConfig {
    fn default() -> Self {
        Self {
            max_rounds: DEFAULT_MAX_RECOVERY_ROUNDS,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ResearchOrchestrator {
    config: ResearchLoopConfig,
}

impl Default for ResearchOrchestrator {
    fn default() -> Self {
        Self::new(ResearchLoopConfig::default())
    }
}

impl ResearchOrchestrator {
    #[must_use]
    pub const fn new(config: ResearchLoopConfig) -> Self {
        Self { config }
    }

    /// 一次主取数后按缺口最多补救 N 轮。网页证据在核心补救完成后加载，避免把慢检索阻塞在
    /// 财务换源循环内；最终评估仍会把证据缺口如实记入 trace。
    pub async fn collect<P: ResearchPorts>(
        &self,
        ports: &P,
        req: &AskRequest,
        route: &ResearchRoute,
    ) -> ResearchFacts {
        let mut facts = ResearchService::assemble_core_facts(ports, req).await;
        self.recover_core(ports, req, route, &mut facts).await;
        facts.evidence = crate::research::load_evidence_for_route(ports, req, &facts, route).await;
        self.finish_trace(route, &mut facts);
        facts
    }

    /// 流式路径先完成核心循环，随后由调用方在发出 evidence stage 后加载网页证据。
    pub(crate) async fn collect_core<P: ResearchPorts>(
        &self,
        ports: &P,
        req: &AskRequest,
        route: &ResearchRoute,
    ) -> ResearchFacts {
        let mut facts = ResearchService::assemble_core_facts(ports, req).await;
        self.recover_core(ports, req, route, &mut facts).await;
        facts
    }

    pub(crate) fn finish_trace(&self, route: &ResearchRoute, facts: &mut ResearchFacts) {
        let final_gaps = evaluate_gaps(route, facts, true);
        facts.recovery.final_gaps = final_gaps;
        facts.recovery.degradation = degradation_for(route, facts);
    }

    async fn recover_core<P: ResearchPorts>(
        &self,
        ports: &P,
        req: &AskRequest,
        route: &ResearchRoute,
        facts: &mut ResearchFacts,
    ) {
        for round in 1..=self.config.max_rounds {
            let requested = evaluate_gaps(route, facts, false);
            if requested.is_empty() {
                break;
            }
            let multiple_type = desired_multiple(&facts.financials);
            let patch = ports
                .recover_missing_facts(FactRecoveryRequest {
                    ticker: req.ticker.clone(),
                    company_name: facts.company.name_zh.clone(),
                    question: req.question.clone(),
                    round,
                    gaps: requested.clone(),
                    multiple_type,
                })
                .await;
            let sources = patch.sources.clone();
            apply_recovery(facts, patch);
            let remaining = evaluate_gaps(route, facts, false);
            let made_progress = remaining != requested;
            facts.recovery.rounds.push(RecoveryRound {
                round,
                requested,
                remaining,
                made_progress,
                sources,
            });
            if !made_progress {
                break;
            }
        }
    }
}

fn intent_needs_financials(intent: ResearchIntent) -> bool {
    matches!(
        intent,
        ResearchIntent::CompanyStatus
            | ResearchIntent::FinancialQuality
            | ResearchIntent::Valuation
            | ResearchIntent::Falsify
            | ResearchIntent::DeepResearch
    )
}

fn intent_needs_valuation(intent: ResearchIntent) -> bool {
    matches!(
        intent,
        ResearchIntent::CompanyStatus
            | ResearchIntent::Valuation
            | ResearchIntent::Falsify
            | ResearchIntent::DeepResearch
    )
}

fn intent_needs_peers(intent: ResearchIntent) -> bool {
    matches!(
        intent,
        ResearchIntent::Competitors | ResearchIntent::Valuation | ResearchIntent::DeepResearch
    )
}

fn intent_needs_filings(intent: ResearchIntent) -> bool {
    matches!(
        intent,
        ResearchIntent::CompanyStatus
            | ResearchIntent::RiskEvent
            | ResearchIntent::Falsify
            | ResearchIntent::DeepResearch
    )
}

#[must_use]
pub fn evaluate_gaps(
    route: &ResearchRoute,
    facts: &ResearchFacts,
    include_evidence: bool,
) -> Vec<ResearchGap> {
    let mut gaps = Vec::new();
    if facts.market.price.is_none() {
        gaps.push(ResearchGap::MarketPrice);
    }

    let needs_financials = intent_needs_financials(route.intent);
    if needs_financials && !facts.financials.provider_ok {
        gaps.push(ResearchGap::FinancialStatements);
    }

    let stage = classify_asset_stage(&facts.financials);
    if intent_needs_valuation(route.intent) && facts.financials.provider_ok {
        match stage {
            AssetStage::Profitable | AssetStage::Unknown => {
                if facts.financials.eps.is_none() || facts.financials.eps_annualized == Some(false)
                {
                    gaps.push(ResearchGap::TtmEps);
                }
            }
            AssetStage::Loss | AssetStage::LossGrowth => {
                if facts.financials.revenue.is_none() {
                    gaps.push(ResearchGap::Revenue);
                }
                if facts.financials.shares_outstanding.is_none() {
                    gaps.push(ResearchGap::SharesOutstanding);
                }
            }
        }
        if facts.financials.historical_valuation.is_none() {
            gaps.push(ResearchGap::HistoricalValuation);
        }
    }
    if intent_needs_peers(route.intent) && facts.peer_anchor.is_none() {
        gaps.push(ResearchGap::PeerComparison);
    }
    if intent_needs_filings(route.intent) && facts.filings.is_empty() {
        gaps.push(ResearchGap::RecentFilings);
    }

    if intent_needs_valuation(route.intent) {
        let panel = build_panel(
            &facts.company,
            &facts.market,
            &facts.financials,
            facts.peer_anchor.as_ref(),
            &facts.filings,
        );
        if !panel.valuation.is_valued() {
            gaps.push(ResearchGap::Valuation);
        }
    }
    if include_evidence
        && echo_domain::intent_wants_web_evidence(route.intent)
        && facts.evidence.is_empty()
    {
        gaps.push(ResearchGap::WebEvidence);
    }
    gaps
}

fn degradation_for(route: &ResearchRoute, facts: &ResearchFacts) -> ResearchDegradation {
    if intent_needs_financials(route.intent) && !facts.financials.provider_ok {
        return ResearchDegradation::QualitativeOnly;
    }
    if intent_needs_valuation(route.intent) {
        let panel = build_panel(
            &facts.company,
            &facts.market,
            &facts.financials,
            facts.peer_anchor.as_ref(),
            &facts.filings,
        );
        if !panel.valuation.is_valued() && facts.peer_anchor.is_some() {
            return ResearchDegradation::PeerComparisonOnly;
        }
    }
    ResearchDegradation::None
}

fn desired_multiple(financials: &Financials) -> MultipleType {
    match classify_asset_stage(financials) {
        AssetStage::Loss | AssetStage::LossGrowth => MultipleType::EvSales,
        _ => MultipleType::Pe,
    }
}

fn apply_recovery(facts: &mut ResearchFacts, patch: FactRecovery) {
    if facts.company.name_zh.is_none() {
        facts.company.name_zh = patch.company_name;
    }
    if let Some(market) = patch.market {
        merge_market(&mut facts.market, market);
    }
    if let Some(loaded) = patch.fundamentals {
        merge_financials(&mut facts.financials, loaded.financials);
        if facts.market.pe.is_none() {
            facts.market.pe = loaded.pe_ttm;
        }
        if facts.company.name_zh.is_none() {
            facts.company.name_zh = loaded.company_name;
        }
    }
    if facts.earnings_calendar.is_none() {
        facts.earnings_calendar = patch.earnings_calendar;
    }
    if facts.financials.historical_valuation.is_none() {
        facts.financials.historical_valuation = patch.historical_valuation;
    }
    if facts.peer_anchor.is_none() {
        facts.peer_anchor = patch.peer_anchor;
    }
    for filing in patch.filings {
        if !facts
            .filings
            .iter()
            .any(|known| known.source_url == filing.source_url)
        {
            facts.filings.push(filing);
        }
    }

    if facts.financials.shares_outstanding.is_none() {
        facts.financials.shares_outstanding = match (facts.market.market_cap, facts.market.price) {
            (Some(cap), Some(price))
                if cap > rust_decimal::Decimal::ZERO && price > rust_decimal::Decimal::ZERO =>
            {
                Some(cap / price)
            }
            _ => None,
        };
    }
}

fn merge_market(target: &mut MarketSnapshot, source: MarketSnapshot) {
    macro_rules! fill {
        ($field:ident) => {
            if target.$field.is_none() {
                target.$field = source.$field;
            }
        };
    }
    fill!(price);
    fill!(pe);
    fill!(market_cap);
    fill!(currency);
    fill!(change_percent);
    fill!(dividend_yield);
}

fn merge_financials(target: &mut Financials, source: Financials) {
    target.provider_ok |= source.provider_ok;
    // 单季/中报 EPS 虽然“有值”，但被明确标成不可年化估值；补救源拿到 TTM/FY 后必须允许
    // 质量升级，否则“缺 TTM EPS → 换 SEC/HKEX 年报”会取到了也永远合不进去。
    let upgrades_eps = (target.eps.is_none() || target.eps_annualized == Some(false))
        && source.eps.is_some()
        && source.eps_annualized != Some(false);
    if upgrades_eps {
        target.eps = source.eps;
        target.eps_annualized = source.eps_annualized;
    }
    macro_rules! fill {
        ($field:ident) => {
            if target.$field.is_none() {
                target.$field = source.$field;
            }
        };
    }
    fill!(eps);
    fill!(eps_annualized);
    fill!(net_margin);
    fill!(operating_margin);
    fill!(revenue);
    fill!(revenue_growth);
    fill!(gross_margin);
    fill!(shares_outstanding);
    fill!(cash_and_equivalents);
    fill!(total_debt);
    fill!(net_cash);
    fill!(free_cash_flow);
    fill!(currency);
    fill!(gross_profit);
    fill!(operating_income);
    fill!(net_income);
    fill!(operating_cash_flow);
    fill!(net_debt);
    fill!(dividend_paid);
    fill!(repurchase_of_stock);
    fill!(pe);
    fill!(pb);
    fill!(return_on_equity);
    fill!(return_on_assets);
    fill!(profit_growth);
    fill!(period);
    fill!(insider_activity);
    if target.hk_buybacks.is_empty() {
        target.hk_buybacks = source.hk_buybacks;
    }
    fill!(historical_valuation);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ResolvedCompany;
    use echo_domain::{Company, ResearchDepth};

    fn route(intent: ResearchIntent) -> ResearchRoute {
        ResearchRoute {
            intent,
            depth: ResearchDepth::Standard,
            confidence: 1.0,
            multi_part: false,
            source: "test",
            answer_style: "test",
            plan: Vec::new(),
        }
    }

    fn empty_facts() -> ResearchFacts {
        ResearchFacts {
            company: ResolvedCompany {
                ticker: "AAPL".into(),
                name_zh: None,
                company: Company::default(),
            },
            market: MarketSnapshot::default(),
            financials: Financials::default(),
            earnings_calendar: None,
            peer_anchor: None,
            filings: Vec::new(),
            evidence: Vec::new(),
            recovery: RecoveryTrace::default(),
        }
    }

    #[test]
    fn valuation_gaps_are_intent_aware() {
        let gaps = evaluate_gaps(&route(ResearchIntent::Valuation), &empty_facts(), false);
        assert!(gaps.contains(&ResearchGap::MarketPrice));
        assert!(gaps.contains(&ResearchGap::FinancialStatements));
        assert!(gaps.contains(&ResearchGap::PeerComparison));
        assert!(gaps.contains(&ResearchGap::Valuation));

        let qualitative = evaluate_gaps(&route(ResearchIntent::Moat), &empty_facts(), false);
        assert_eq!(qualitative, vec![ResearchGap::MarketPrice]);
    }
}
