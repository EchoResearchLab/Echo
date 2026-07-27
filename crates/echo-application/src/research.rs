//! 研究用例编排——API 只做 HTTP，事实组装 / 估值 / 生成 / 护栏 / 落库收口到这里。
//!
//! 编排顺序固定为：解析主体（当前由请求 ticker 提供）→ 取数 → 衍生/估值 →
//! 提示词 → 生成 → 护栏 → 持久化。端口用假实现即可做无 IO 单测。

use crate::answer_prompt::{
    AnswerContext, CompareLegContext, build_compare_user_prompt, build_system_prompt,
    build_user_prompt,
};
use crate::model_gateway::{ModelStreamEvent, ModelStreamStart};
use crate::{DecisionPanel, ResolvedCompany, build_panel};
use echo_contracts::{
    AnswerSource, AskRequest, AskResponse, AssetStageView, CitationGuardView, CompanyHeaderView,
    CompareLegView, CompareResponse, EarningsCalendarView, EvidenceView, FilingView, GuardView,
    MethodBandView, ResearchStreamCompare, ResearchStreamDelta, ResearchStreamError,
    ResearchStreamEvent, ResearchStreamFinal, ResearchStreamGuard, ResearchStreamMeta,
    ResearchStreamStage, ResearchStreamStageName, RouteView, ValuationView,
};
use echo_domain::{
    AssetStage, Company, EarningsCalendar, Evidence, Filing, Financials, HistoricalValuation,
    MarketSnapshot, MultipleType, PeerAnchor, RegistrySources, ResearchRoute, build_facts_registry,
    build_soft_note, classify_asset_stage, intent_wants_web_evidence, route_research_intent,
    verify_answer_citations, verify_answer_numbers,
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::future::Future;
use tokio::sync::mpsc;

pub use echo_domain::intent::{
    ResearchDepth, ResearchIntent, classify_research_intent, plan_research_stages,
    route_research_intent as route_intent,
};

/// 单公司研究事实——比较场景用 [`CompareResearchFacts`] 隔离第二家。
#[derive(Clone, Debug)]
pub struct ResearchFacts {
    pub company: ResolvedCompany,
    pub market: MarketSnapshot,
    pub financials: Financials,
    pub earnings_calendar: Option<EarningsCalendar>,
    pub peer_anchor: Option<PeerAnchor>,
    pub filings: Vec<Filing>,
    /// 网页证据（仅定性意图拉取，数字驱动意图与失败降级时为空）——只做定性支撑，
    /// 绝不进 `FactsRegistry`（护栏只认一手财报）。
    pub evidence: Vec<Evidence>,
}

/// 比较研究的双腿事实；两侧 registry 不得交叉污染。
#[derive(Clone, Debug)]
pub struct CompareResearchFacts {
    pub primary: ResearchFacts,
    pub peer: ResearchFacts,
}

/// 同一研究会话此前一轮的问答摘要——只用于代词/实体承接，绝不作为本轮数字核对依据
/// （不携带任何 `FactsRegistry` 数值，只是问题原文 + 上轮作答的自然语言文本）。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PriorTurn {
    pub question: String,
    pub answer: String,
}

/// 会话落库所需最小字段（与 `echo-db::SaveResearchSession` 对齐，避免端口泄漏 sqlx 细节）。
#[derive(Clone, Debug, Default)]
pub struct PersistResearchSession {
    /// 续问同一会话时带上已有 id，落库归位同一行而不是插入新行。
    pub id: Option<String>,
    pub ticker: String,
    pub company_name: Option<String>,
    pub question: Option<String>,
    pub report_markdown: Option<String>,
    pub decision_panel: Option<serde_json::Value>,
    pub full_research: Option<String>,
    pub data_sources: Option<serde_json::Value>,
    pub turn_count: Option<i32>,
    /// 累积问答历史（含本轮）；只有生成了新作答时才带上，否则 `None` 保留库里原值。
    pub thread: Option<serde_json::Value>,
}

/// 外部财务事实补数结果——请求体缺数时由端口注入；`pe_ttm` 优先于行情 PE。
#[derive(Clone, Debug, Default)]
pub struct LoadedFundamentals {
    pub financials: Financials,
    pub pe_ttm: Option<rust_decimal::Decimal>,
    pub company_name: Option<String>,
}

/// 研究副作用端口——DB / 行情 / 财务 / 模型 / 落库都从这里注入。
pub trait ResearchPorts: Send + Sync {
    fn load_company_market(
        &self,
        ticker: &str,
    ) -> impl Future<Output = Option<(ResolvedCompany, MarketSnapshot)>> + Send;

    fn refresh_quote(&self, ticker: &str) -> impl Future<Output = Result<(), String>> + Send;

    /// 请求体未带财务数字时补数。失败/缺源返回 `None`（保持未核到）。
    fn load_fundamentals(
        &self,
        ticker: &str,
    ) -> impl Future<Output = Option<LoadedFundamentals>> + Send;

    /// 下一次财报日历。缺数/未核到返回 `None`——绝不占位。
    fn load_earnings_calendar(
        &self,
        ticker: &str,
    ) -> impl Future<Output = Option<EarningsCalendar>> + Send;

    /// 历史估值分位（美股专属）。缺数/未核到返回 `None`——绝不占位。
    fn load_historical_valuation(
        &self,
        ticker: &str,
    ) -> impl Future<Output = Option<HistoricalValuation>> + Send;

    /// 同业锚点（美股专属）。`multiple_type` 由调用方按公司自身资产阶段（盈利/亏损）先行
    /// 判定——同一批可比公司同时有 PE 与 EV/Sales 两种口径，缺数/未核到返回 `None`。
    fn load_peer_anchor(
        &self,
        ticker: &str,
        multiple_type: MultipleType,
    ) -> impl Future<Output = Option<PeerAnchor>> + Send;

    /// 最近公司公告/披露（美股专属）。缺数/未核到返回空列表——绝不占位。
    fn load_recent_filings(&self, ticker: &str) -> impl Future<Output = Vec<Filing>> + Send;

    /// 网页证据（定性意图专属，美股/港股皆可）。`name` 传已核到的公司名以提升检索质量。
    /// 未配供应商/商用拒绝/网络失败一律返回空列表——绝不占位，让链路诚实降级。
    fn load_web_evidence(
        &self,
        ticker: &str,
        name: Option<&str>,
        question: &str,
    ) -> impl Future<Output = Vec<Evidence>> + Send;

    /// 续问已有会话时读回此前几轮问答（仅供代词/实体承接）。会话不存在/不属于该用户/
    /// 未带 `session_id` 一律返回空——空历史不是错误，是"这是新会话"。
    fn load_prior_turns(
        &self,
        user_id: &str,
        session_id: &str,
    ) -> impl Future<Output = Vec<PriorTurn>> + Send;

    fn complete_answer(
        &self,
        system: &str,
        user: &str,
        user_id: &str,
    ) -> impl Future<Output = Option<String>> + Send;

    /// 流式作答。返回 [`ModelStreamStart::Unavailable`] 时编排层走 structured unavailable。
    fn stream_answer(
        &self,
        system: String,
        user: String,
        user_id: String,
    ) -> impl Future<Output = ModelStreamStart> + Send;

    /// 落库并返回归位的会话 id（续问时与传入的 `id` 一致，新会话时是新生成的 id）。
    fn save_session(
        &self,
        user_id: &str,
        session: PersistResearchSession,
    ) -> impl Future<Output = Result<String, String>> + Send;
}

/// 一次非流式研究的结果；可追溯主体、护栏与是否已落库。
#[derive(Clone, Debug)]
pub struct ResearchOutcome {
    pub response: AskResponse,
    pub facts: ResearchFacts,
    pub route: ResearchRoute,
    pub persisted: bool,
}

/// 一次对比研究的结果；两腿事实全程隔离，不落库（对比会话的落库形态待产品判断）。
#[derive(Clone, Debug)]
pub struct CompareOutcome {
    pub response: CompareResponse,
    pub facts: CompareResearchFacts,
    pub route: ResearchRoute,
}

#[derive(Clone, Debug)]
pub struct ResearchService;

impl ResearchService {
    /// 组装结构化核心事实；网页证据单独在下一阶段加载，使流式路径能在真实 IO 开始前发出
    /// `evidence` 进度，而不是所有取数结束后才假装切换阶段。
    async fn assemble_core_facts<P: ResearchPorts>(ports: &P, req: &AskRequest) -> ResearchFacts {
        let mut company = ResolvedCompany {
            ticker: req.ticker.clone(),
            name_zh: req.name_zh.clone(),
            company: Company {
                price: req.price,
                pe: req.pe,
                ..Default::default()
            },
        };
        let mut market = MarketSnapshot {
            price: req.price,
            pe: req.pe,
            market_cap: req.market_cap,
            currency: req.quote_currency.clone(),
            change_percent: req.change_percent,
            ..Default::default()
        };
        if market.price.is_none() {
            if let Some((resolved, snapshot)) = ports.load_company_market(&req.ticker).await {
                company = resolved;
                market = snapshot;
            } else if ports.refresh_quote(&req.ticker).await.is_ok() {
                if let Some((resolved, snapshot)) = ports.load_company_market(&req.ticker).await {
                    company = resolved;
                    market = snapshot;
                }
            }
        }
        let mut financials = Financials {
            provider_ok: req.revenue.is_some() || req.eps.is_some(),
            eps: req.eps,
            eps_annualized: req.eps_annualized,
            net_margin: req.net_margin,
            gross_margin: req.gross_margin,
            revenue: req.revenue,
            revenue_growth: req.revenue_growth,
            net_income: req.net_income,
            shares_outstanding: req.shares_outstanding,
            free_cash_flow: req.free_cash_flow,
            net_cash: req.net_cash,
            currency: req.reporting_currency.clone(),
            ..Default::default()
        };
        if !financials.provider_ok {
            if let Some(loaded) = ports.load_fundamentals(&req.ticker).await {
                financials = loaded.financials;
                if market.pe.is_none() {
                    market.pe = loaded.pe_ttm;
                }
                if company.name_zh.is_none() {
                    company.name_zh = loaded.company_name;
                }
            }
        }
        // EV/Sales 需要股本。供应商未直接给股本时，可由同一行情快照的市值/股价反推；
        // 两者同属报价币种，商相除后是股数，不引入汇率混用。
        if financials.shares_outstanding.is_none() {
            financials.shares_outstanding = match (market.market_cap, market.price) {
                (Some(market_cap), Some(price))
                    if market_cap > Decimal::ZERO && price > Decimal::ZERO =>
                {
                    Some(market_cap / price)
                }
                _ => None,
            };
        }
        financials.historical_valuation = ports.load_historical_valuation(&req.ticker).await;
        let earnings_calendar = ports.load_earnings_calendar(&req.ticker).await;
        // 亏损股走 EV/Sales 情景（PE 对负利润无意义），其余走 PE 带；与 `compute_valuation`
        // 的阶段判定同一口径，否则取回的锚点类型会被域层过滤器悄悄丢弃。
        let multiple_type = match classify_asset_stage(&financials) {
            AssetStage::Loss | AssetStage::LossGrowth => MultipleType::EvSales,
            _ => MultipleType::Pe,
        };
        let peer_anchor = ports.load_peer_anchor(&req.ticker, multiple_type).await;
        let filings = ports.load_recent_filings(&req.ticker).await;
        ResearchFacts {
            company,
            market,
            financials,
            earnings_calendar,
            peer_anchor,
            filings,
            evidence: Vec::new(),
        }
    }

    /// 完整事实组装：核心行情/财报完成后，按路由计划加载定性网页证据。
    pub async fn assemble_facts<P: ResearchPorts>(ports: &P, req: &AskRequest) -> ResearchFacts {
        let route = route_research_intent(&req.question);
        let mut facts = Self::assemble_core_facts(ports, req).await;
        facts.evidence = load_evidence_for_route(ports, req, &facts, &route).await;
        facts
    }

    /// 非流式研究主用例。
    pub async fn ask<P: ResearchPorts>(
        ports: &P,
        user_id: &str,
        req: AskRequest,
    ) -> ResearchOutcome {
        let route = route_research_intent(&req.question);
        let facts = Self::assemble_facts(ports, &req).await;
        let panel = build_panel(
            &facts.company,
            &facts.market,
            &facts.financials,
            facts.peer_anchor.as_ref(),
            &facts.filings,
        );
        let prior_turns = load_prior_turns_for(ports, user_id, &req).await;

        let (answer, answer_source) = match req.draft_answer.clone() {
            Some(draft) => (Some(draft), AnswerSource::Draft),
            None => {
                let system = build_system_prompt();
                let user_prompt = build_user_prompt(&AnswerContext {
                    question: &req.question,
                    name_zh: facts.company.name_zh.as_deref(),
                    panel: &panel,
                    market: &facts.market,
                    financials: &facts.financials,
                    filings: &facts.filings,
                    evidence: &facts.evidence,
                    depth: route.depth,
                    history: &prior_turns,
                });
                match ports.complete_answer(&system, &user_prompt, user_id).await {
                    Some(generated) => (Some(generated), AnswerSource::Generated),
                    None => (None, AnswerSource::Unavailable),
                }
            }
        };

        let fact_guard = answer
            .as_deref()
            .map(|draft| guard_view(&req, &facts, &panel, draft));
        let citation_guard = answer
            .as_deref()
            .and_then(|draft| citation_guard_view(&facts.evidence, draft));

        let mut response = build_ask_response(
            &panel,
            company_header(&panel.ticker, &facts.company, &facts.market),
            &route,
            answer,
            answer_source,
            fact_guard,
            citation_guard,
            facts.earnings_calendar.as_ref(),
            &facts.filings,
            &facts.evidence,
        );
        let (persisted, session_id) =
            persist_outcome(ports, user_id, &req, &facts, &response, &prior_turns).await;
        response.session_id = session_id;

        ResearchOutcome {
            response,
            facts,
            route,
            persisted,
        }
    }

    /// 双主体对比研究：两腿各自独立走 `assemble_facts`（同一条单公司取数管线，互不知道
    /// 对方存在），生成阶段才把两份已核事实一起摆给模型看；护栏阶段"分别验证"——每腿只用
    /// 自己的 `FactsRegistry` 核对整段作答，绝不合并两份登记表（合并会把"腾讯的营收"变成
    /// 对"苹果营收"合法的核对来源，是架构上明令禁止的"问苹果答腾讯"污染）。
    pub async fn compare<P: ResearchPorts>(
        ports: &P,
        user_id: &str,
        question: String,
        primary_ticker: String,
        peer_ticker: String,
    ) -> CompareOutcome {
        let route = route_research_intent(&question);
        let primary_req = AskRequest::minimal(question.clone(), primary_ticker);
        let peer_req = AskRequest::minimal(question.clone(), peer_ticker);
        let primary_facts = Self::assemble_facts(ports, &primary_req).await;
        let peer_facts = Self::assemble_facts(ports, &peer_req).await;
        let primary_panel = build_panel(
            &primary_facts.company,
            &primary_facts.market,
            &primary_facts.financials,
            primary_facts.peer_anchor.as_ref(),
            &primary_facts.filings,
        );
        let peer_panel = build_panel(
            &peer_facts.company,
            &peer_facts.market,
            &peer_facts.financials,
            peer_facts.peer_anchor.as_ref(),
            &peer_facts.filings,
        );

        let system = build_system_prompt();
        let user_prompt = build_compare_user_prompt(
            &question,
            &CompareLegContext {
                name_zh: primary_facts.company.name_zh.as_deref(),
                panel: &primary_panel,
                market: &primary_facts.market,
                financials: &primary_facts.financials,
                evidence: &primary_facts.evidence,
            },
            &CompareLegContext {
                name_zh: peer_facts.company.name_zh.as_deref(),
                panel: &peer_panel,
                market: &peer_facts.market,
                financials: &peer_facts.financials,
                evidence: &peer_facts.evidence,
            },
        );
        let (answer, answer_source) =
            match ports.complete_answer(&system, &user_prompt, user_id).await {
                Some(generated) => (Some(generated), AnswerSource::Generated),
                None => (None, AnswerSource::Unavailable),
            };

        // 分别验证：每腿只吃自己的登记表核对同一段作答，互不借用。
        let primary_guard = answer
            .as_deref()
            .map(|draft| guard_view(&primary_req, &primary_facts, &primary_panel, draft));
        let peer_guard = answer
            .as_deref()
            .map(|draft| guard_view(&peer_req, &peer_facts, &peer_panel, draft));

        let response = CompareResponse {
            route: route_view(&route),
            primary: compare_leg_view(&primary_panel, primary_guard, &primary_facts.evidence),
            peer: compare_leg_view(&peer_panel, peer_guard, &peer_facts.evidence),
            answer,
            answer_source,
        };

        CompareOutcome {
            response,
            facts: CompareResearchFacts {
                primary: primary_facts,
                peer: peer_facts,
            },
            route,
        }
    }

    /// 对话内双主体对比的流式外壳：`stage(assembling)` → `compare`。对比本身无逐字流
    /// （模型作答是单次调用），走与问答同一条 SSE 通道让 Web 复用 thread/取消/超时机制。
    pub fn compare_stream<P>(
        ports: P,
        user_id: String,
        question: String,
        primary_ticker: String,
        peer_ticker: String,
    ) -> mpsc::Receiver<ResearchStreamEvent>
    where
        P: ResearchPorts + Send + Sync + 'static,
    {
        let (tx, rx) = mpsc::channel(8);
        tokio::spawn(async move {
            let _ = tx
                .send(ResearchStreamEvent::Stage(ResearchStreamStage {
                    name: ResearchStreamStageName::Assembling,
                    index: 0,
                    total: 0,
                }))
                .await;
            let outcome =
                Self::compare(&ports, &user_id, question, primary_ticker, peer_ticker).await;
            let _ = tx
                .send(ResearchStreamEvent::Compare(Box::new(
                    ResearchStreamCompare {
                        response: outcome.response,
                    },
                )))
                .await;
        });
        rx
    }

    /// 类型化流式研究：先按 `route.plan` 发精确 stage，随后 meta/delta/guard/final；
    /// 模型失败则 error 且不落库。
    pub fn ask_stream<P>(
        ports: P,
        user_id: String,
        req: AskRequest,
    ) -> mpsc::Receiver<ResearchStreamEvent>
    where
        P: ResearchPorts + Send + Sync + 'static,
    {
        let (tx, rx) = mpsc::channel(32);
        tokio::spawn(async move {
            if let Err(message) = drive_stream(&ports, &user_id, req, &tx).await {
                let _ = tx
                    .send(ResearchStreamEvent::Error(ResearchStreamError { message }))
                    .await;
            }
        });
        rx
    }
}

async fn drive_stream<P: ResearchPorts>(
    ports: &P,
    user_id: &str,
    req: AskRequest,
    tx: &mpsc::Sender<ResearchStreamEvent>,
) -> Result<(), String> {
    let send = |event: ResearchStreamEvent| async {
        tx.send(event)
            .await
            .map_err(|_| "stream consumer dropped".to_string())
    };

    let route = route_research_intent(&req.question);
    for step in ["routing", "resolving", "market_financials"] {
        send(ResearchStreamEvent::Stage(
            plan_stream_stage(&route, step).expect("base route plan contains core stage"),
        ))
        .await?;
    }

    let mut facts = ResearchService::assemble_core_facts(ports, &req).await;
    if let Some(stage) = plan_stream_stage(&route, "evidence") {
        send(ResearchStreamEvent::Stage(stage)).await?;
        facts.evidence = load_evidence_for_route(ports, &req, &facts, &route).await;
    }
    if let Some(stage) = plan_stream_stage(&route, "valuation") {
        send(ResearchStreamEvent::Stage(stage)).await?;
    }
    let panel = build_panel(
        &facts.company,
        &facts.market,
        &facts.financials,
        facts.peer_anchor.as_ref(),
        &facts.filings,
    );
    let prior_turns = load_prior_turns_for(ports, user_id, &req).await;

    send(ResearchStreamEvent::Meta(Box::new(ResearchStreamMeta {
        ticker: panel.ticker.clone(),
        company: company_header(&panel.ticker, &facts.company, &facts.market),
        route: route_view(&route),
        data_completeness: panel.data_completeness,
        connected_sources: panel
            .connected_sources
            .iter()
            .map(|s| (*s).to_string())
            .collect(),
        valuation: valuation_view(&panel),
        earnings: earnings_view(facts.earnings_calendar.as_ref()),
    })))
    .await?;

    let (answer, answer_source) = if let Some(draft) = req.draft_answer.clone() {
        send(ResearchStreamEvent::Stage(
            plan_stream_stage(&route, "generating").expect("route plan contains generating"),
        ))
        .await?;
        send(ResearchStreamEvent::Delta(ResearchStreamDelta {
            text: draft.clone(),
        }))
        .await?;
        (Some(draft), AnswerSource::Draft)
    } else {
        send(ResearchStreamEvent::Stage(
            plan_stream_stage(&route, "generating").expect("route plan contains generating"),
        ))
        .await?;
        let system = build_system_prompt();
        let user_prompt = build_user_prompt(&AnswerContext {
            question: &req.question,
            name_zh: facts.company.name_zh.as_deref(),
            panel: &panel,
            market: &facts.market,
            financials: &facts.financials,
            filings: &facts.filings,
            evidence: &facts.evidence,
            depth: route.depth,
            history: &prior_turns,
        });
        match ports
            .stream_answer(system, user_prompt, user_id.to_string())
            .await
        {
            ModelStreamStart::Unavailable => (None, AnswerSource::Unavailable),
            ModelStreamStart::Ready(mut model_rx) => {
                let mut accumulated = String::new();
                while let Some(event) = model_rx.recv().await {
                    match event {
                        ModelStreamEvent::Delta(text) => {
                            accumulated.push_str(&text);
                            send(ResearchStreamEvent::Delta(ResearchStreamDelta { text })).await?;
                        }
                        ModelStreamEvent::Completed => {
                            break;
                        }
                        ModelStreamEvent::Failed { message } => {
                            return Err(message);
                        }
                    }
                }
                if accumulated.is_empty() {
                    (None, AnswerSource::Unavailable)
                } else {
                    (Some(accumulated), AnswerSource::Generated)
                }
            }
        }
    };

    send(ResearchStreamEvent::Stage(
        plan_stream_stage(&route, "fact_check").expect("route plan contains fact_check"),
    ))
    .await?;

    let fact_guard = answer
        .as_deref()
        .map(|draft| guard_view(&req, &facts, &panel, draft));
    let citation_guard = answer
        .as_deref()
        .and_then(|draft| citation_guard_view(&facts.evidence, draft));
    send(ResearchStreamEvent::Guard(ResearchStreamGuard {
        fact_guard: fact_guard.clone(),
        citation_guard: citation_guard.clone(),
    }))
    .await?;

    let mut response = build_ask_response(
        &panel,
        company_header(&panel.ticker, &facts.company, &facts.market),
        &route,
        answer,
        answer_source,
        fact_guard,
        citation_guard,
        facts.earnings_calendar.as_ref(),
        &facts.filings,
        &facts.evidence,
    );

    let (persisted, session_id) =
        persist_outcome(ports, user_id, &req, &facts, &response, &prior_turns).await;
    response.session_id = session_id;
    send(ResearchStreamEvent::Final(Box::new(ResearchStreamFinal {
        response,
        persisted,
    })))
    .await?;
    Ok(())
}

fn plan_stream_stage(route: &ResearchRoute, step: &str) -> Option<ResearchStreamStage> {
    let index = route.plan.iter().position(|candidate| *candidate == step)? + 1;
    let name = match step {
        "routing" => ResearchStreamStageName::Routing,
        "resolving" => ResearchStreamStageName::Resolving,
        "market_financials" => ResearchStreamStageName::MarketFinancials,
        "evidence" => ResearchStreamStageName::Evidence,
        "valuation" => ResearchStreamStageName::Valuation,
        "generating" => ResearchStreamStageName::Generating,
        "fact_check" => ResearchStreamStageName::FactCheck,
        _ => return None,
    };
    Some(ResearchStreamStage {
        name,
        index,
        total: route.plan.len(),
    })
}

/// 有 `session_id` 才读历史；新会话没有可承接的上文，空列表就是正确答案。
pub(crate) async fn load_prior_turns_for<P: ResearchPorts>(
    ports: &P,
    user_id: &str,
    req: &AskRequest,
) -> Vec<PriorTurn> {
    match req.session_id.as_deref() {
        Some(session_id) => ports.load_prior_turns(user_id, session_id).await,
        None => Vec::new(),
    }
}

/// deep 研究的额外检索维度——"多步检索"的那几步：护城河/竞争、风险/监管、最新动态/业绩。
/// 在基础问题之外各拉一批，union 去重后喂给综合。非 deep 不用。
const DEEP_EVIDENCE_ASPECTS: &[&str] = &[
    "护城河 竞争壁垒 定价权",
    "风险 监管 诉讼 做空 业绩不及预期",
    "最新 业绩 战略 进展",
];
/// deep 合并去重后保留的证据条数上限——控制提示词体量与成本。
const MAX_DEEP_EVIDENCE: usize = 10;

async fn load_evidence_for_route<P: ResearchPorts>(
    ports: &P,
    req: &AskRequest,
    facts: &ResearchFacts,
    route: &ResearchRoute,
) -> Vec<Evidence> {
    if !intent_wants_web_evidence(route.intent) {
        return Vec::new();
    }
    gather_evidence(
        ports,
        &req.ticker,
        facts.company.name_zh.as_deref(),
        &req.question,
        route.depth,
    )
    .await
}

/// 按深度决定证据检索广度：`Deep` 走基础问题 + 多维**并发**检索、按 URL 去重合并、截断到
/// [`MAX_DEEP_EVIDENCE`]；其余深度只用基础问题一条。这是"deep 走多步检索→综合"里的"多步检索"，
/// 综合仍由生成阶段一次完成（更厚的证据集喂给模型）。
pub(crate) async fn gather_evidence<P: ResearchPorts>(
    ports: &P,
    ticker: &str,
    name: Option<&str>,
    question: &str,
    depth: ResearchDepth,
) -> Vec<Evidence> {
    if depth != ResearchDepth::Deep {
        return ports.load_web_evidence(ticker, name, question).await;
    }
    let mut queries: Vec<String> = vec![question.to_string()];
    for aspect in DEEP_EVIDENCE_ASPECTS {
        queries.push(format!("{question} {aspect}"));
    }
    let batches = futures_util::future::join_all(
        queries
            .iter()
            .map(|q| ports.load_web_evidence(ticker, name, q)),
    )
    .await;
    let mut seen = std::collections::HashSet::new();
    let mut merged = Vec::new();
    for batch in batches {
        for ev in batch {
            if seen.insert(ev.url.clone()) {
                merged.push(ev);
            }
        }
    }
    merged.truncate(MAX_DEEP_EVIDENCE);
    merged
}

pub(crate) fn guard_view(
    req: &AskRequest,
    facts: &ResearchFacts,
    panel: &DecisionPanel,
    draft: &str,
) -> GuardView {
    let registry = build_facts_registry(&RegistrySources {
        ticker: &req.ticker,
        native_currency: req
            .reporting_currency
            .as_deref()
            .or(req.quote_currency.as_deref()),
        market: Some(&facts.market),
        financials: Some(&facts.financials),
        valuation: Some(&panel.valuation),
        earnings_next_date: facts
            .earnings_calendar
            .as_ref()
            .and_then(|c| c.next_date.as_deref()),
        // 公告随事实块一起喂给了模型，日期就必须可核。
        filings: &facts.filings,
        ..Default::default()
    });
    let report = verify_answer_numbers(draft, &registry);
    GuardView {
        total: report.checked.len(),
        pass: report.pass_count(),
        soft: report.soft_count,
        hard: report.hard_count,
        has_hard_fail: report.has_hard_fail(),
        soft_note: build_soft_note(&report),
    }
}

/// 定性引用护栏——本轮有网页证据时才产出。核引用完整性（标注了几号来源、有无虚构来源号、
/// 有证据却零引用），与数字护栏互补。无证据（`evidence.is_empty()`）返回 `None`：不苛求引用。
pub(crate) fn citation_guard_view(evidence: &[Evidence], draft: &str) -> Option<CitationGuardView> {
    if evidence.is_empty() {
        return None;
    }
    let report = verify_answer_citations(draft, evidence.len());
    let note = if report.has_hallucinated_citation() {
        format!(
            "引用了不存在的来源号 {}，已标记",
            report
                .out_of_range
                .iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join("、")
        )
    } else if report.ungrounded {
        "定性论断未标注任何来源，置信度下降".to_string()
    } else {
        String::new()
    };
    Some(CitationGuardView {
        evidence_count: report.evidence_count,
        cited_count: report.cited_count(),
        out_of_range: report.out_of_range.len(),
        ungrounded: report.ungrounded,
        has_hard_fail: report.has_hallucinated_citation(),
        note,
    })
}

#[allow(clippy::too_many_arguments)]
/// 行情抬头。**全缺就返回 `None`**——一个只有代码、没有任何数字的抬头是纯噪声，
/// 不如不画；缺哪一项就少哪一项，绝不用 0 或上一次的值占位。
fn company_header(
    ticker: &str,
    company: &ResolvedCompany,
    market: &MarketSnapshot,
) -> Option<CompanyHeaderView> {
    let name = company.name_zh.clone();
    let header = CompanyHeaderView {
        ticker: ticker.to_string(),
        name,
        price: market.price,
        change_percent: market.change_percent,
        market_cap: market.market_cap,
        currency: market.currency.clone(),
    };
    let empty = header.name.is_none()
        && header.price.is_none()
        && header.change_percent.is_none()
        && header.market_cap.is_none();
    (!empty).then_some(header)
}

#[allow(clippy::too_many_arguments)]
fn build_ask_response(
    panel: &DecisionPanel,
    company: Option<CompanyHeaderView>,
    route: &ResearchRoute,
    answer: Option<String>,
    answer_source: AnswerSource,
    fact_guard: Option<GuardView>,
    citation_guard: Option<CitationGuardView>,
    earnings_calendar: Option<&EarningsCalendar>,
    filings: &[Filing],
    evidence: &[Evidence],
) -> AskResponse {
    AskResponse {
        ticker: panel.ticker.clone(),
        company,
        route: route_view(route),
        data_completeness: panel.data_completeness,
        connected_sources: panel
            .connected_sources
            .iter()
            .map(|s| (*s).to_string())
            .collect(),
        valuation: valuation_view(panel),
        answer,
        answer_source,
        fact_guard,
        citation_guard,
        earnings: earnings_view(earnings_calendar),
        filings: filings_view(filings),
        sources: sources_view(evidence),
        session_id: None,
    }
}

/// 证据领域类型 → 契约来源卡视图。
pub(crate) fn sources_view(evidence: &[Evidence]) -> Vec<EvidenceView> {
    evidence
        .iter()
        .map(|e| EvidenceView {
            title: e.title.clone(),
            url: e.url.clone(),
            snippet: e.snippet.clone(),
            published_date: e.published_date.clone(),
            source_domain: e.source_domain.clone(),
        })
        .collect()
}

pub(crate) fn filings_view(filings: &[Filing]) -> Vec<FilingView> {
    filings
        .iter()
        .map(|filing| FilingView {
            form: filing.form.clone(),
            filed_date: filing.filed_date.clone(),
            source_url: filing.source_url.clone(),
        })
        .collect()
}

/// 缺数（`provider_ok == false`）即不进响应，绝不用空壳字段冒充"已核到"。
pub(crate) fn earnings_view(calendar: Option<&EarningsCalendar>) -> Option<EarningsCalendarView> {
    let calendar = calendar.filter(|c| c.provider_ok)?;
    Some(EarningsCalendarView {
        next_date: calendar.next_date.clone(),
        quarter: calendar.quarter,
        year: calendar.year,
        eps_estimate: calendar.eps_estimate,
        revenue_estimate: calendar.revenue_estimate,
    })
}

/// 落库并返回 `(是否成功, 归位的会话 id)`；失败时若是续问已有会话，id 原样带回（客户端
/// 已经在跟踪那个会话，不因这一轮落库失败就丢线索）。
async fn persist_outcome<P: ResearchPorts>(
    ports: &P,
    user_id: &str,
    req: &AskRequest,
    facts: &ResearchFacts,
    response: &AskResponse,
    prior_turns: &[PriorTurn],
) -> (bool, Option<String>) {
    let thread = response.answer.as_ref().map(|answer| {
        let mut entries = prior_turns.to_vec();
        entries.push(PriorTurn {
            question: req.question.clone(),
            answer: answer.clone(),
        });
        serde_json::to_value(&entries).unwrap_or(serde_json::Value::Null)
    });
    let session = PersistResearchSession {
        id: req.session_id.clone(),
        ticker: req.ticker.clone(),
        company_name: facts.company.name_zh.clone(),
        question: Some(req.question.clone()),
        report_markdown: response.answer.clone(),
        decision_panel: serde_json::to_value(&response.valuation).ok(),
        full_research: response.answer.clone(),
        data_sources: Some(serde_json::json!({ "connected": response.connected_sources.clone() })),
        turn_count: Some(prior_turns.len() as i32 + 1),
        thread,
    };
    match ports.save_session(user_id, session).await {
        Ok(id) => (true, Some(id)),
        Err(_) => (false, req.session_id.clone()),
    }
}

pub(crate) fn route_view(route: &ResearchRoute) -> RouteView {
    RouteView {
        intent: route.intent.as_str().to_string(),
        depth: route.depth.as_str().to_string(),
        confidence: route.confidence,
        multi_part: route.multi_part,
        answer_style: route.answer_style.to_string(),
        plan: route.plan.iter().map(|s| (*s).to_string()).collect(),
    }
}

fn compare_leg_view(
    panel: &DecisionPanel,
    fact_guard: Option<GuardView>,
    evidence: &[Evidence],
) -> CompareLegView {
    CompareLegView {
        ticker: panel.ticker.clone(),
        data_completeness: panel.data_completeness,
        connected_sources: panel
            .connected_sources
            .iter()
            .map(|s| (*s).to_string())
            .collect(),
        valuation: valuation_view(panel),
        sources: sources_view(evidence),
        fact_guard,
    }
}

pub(crate) fn valuation_view(panel: &DecisionPanel) -> ValuationView {
    let valuation = &panel.valuation;
    let stage = valuation.stage.map(|stage| match stage {
        AssetStage::Profitable => AssetStageView::Profitable,
        AssetStage::LossGrowth => AssetStageView::LossGrowth,
        AssetStage::Loss => AssetStageView::Loss,
        AssetStage::Unknown => AssetStageView::Unknown,
    });
    ValuationView {
        method: valuation.method.clone(),
        bear: valuation.bear,
        base: valuation.base,
        bull: valuation.bull,
        upside: valuation.upside.clone(),
        downside: valuation.downside.clone(),
        current_price: valuation.current_price,
        methods: valuation.methods.clone(),
        method_detail: valuation
            .method_detail
            .iter()
            .map(|band| MethodBandView {
                name: band.name.clone(),
                bear: band.bear,
                base: band.base,
                bull: band.bull,
            })
            .collect(),
        key_assumptions: valuation.key_assumptions.clone(),
        sensitivity: valuation.sensitivity.clone(),
        stage_aware: valuation.stage_aware,
        stage,
        data_suspect: valuation.data_suspect,
        cannot_value_reason: valuation.cannot_value_reason.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakePorts {
        market: Option<(ResolvedCompany, MarketSnapshot)>,
        /// 按 ticker 区分的行情/主体——只给对比研究测试用，命中优先于 `market`。
        market_by_ticker: std::collections::HashMap<String, (ResolvedCompany, MarketSnapshot)>,
        refresh_ok: bool,
        fundamentals: Option<LoadedFundamentals>,
        /// 按 ticker 区分的财报——只给对比研究测试用，命中优先于 `fundamentals`。
        fundamentals_by_ticker: std::collections::HashMap<String, LoadedFundamentals>,
        earnings_calendar: Option<EarningsCalendar>,
        historical_valuation: Option<HistoricalValuation>,
        answer: Option<String>,
        stream_chunks: Vec<String>,
        stream_fail: Option<String>,
        stream_unavailable: bool,
        saved: Mutex<Vec<PersistResearchSession>>,
        fail_save: bool,
        prior_turns: Vec<PriorTurn>,
        evidence: Vec<Evidence>,
        /// `load_web_evidence` 被调用次数——测多维检索广度（deep 多次、其余一次）。
        evidence_calls: std::sync::atomic::AtomicUsize,
    }

    impl ResearchPorts for FakePorts {
        async fn load_company_market(
            &self,
            ticker: &str,
        ) -> Option<(ResolvedCompany, MarketSnapshot)> {
            self.market_by_ticker
                .get(ticker)
                .cloned()
                .or_else(|| self.market.clone())
        }

        async fn refresh_quote(&self, _ticker: &str) -> Result<(), String> {
            if self.refresh_ok {
                Ok(())
            } else {
                Err("unavailable".into())
            }
        }

        async fn load_fundamentals(&self, ticker: &str) -> Option<LoadedFundamentals> {
            self.fundamentals_by_ticker
                .get(ticker)
                .cloned()
                .or_else(|| self.fundamentals.clone())
        }

        async fn load_earnings_calendar(&self, _ticker: &str) -> Option<EarningsCalendar> {
            self.earnings_calendar.clone()
        }

        async fn load_historical_valuation(&self, _ticker: &str) -> Option<HistoricalValuation> {
            self.historical_valuation.clone()
        }

        async fn load_peer_anchor(
            &self,
            _ticker: &str,
            _multiple_type: MultipleType,
        ) -> Option<PeerAnchor> {
            None
        }

        async fn load_recent_filings(&self, _ticker: &str) -> Vec<Filing> {
            Vec::new()
        }

        async fn load_web_evidence(
            &self,
            _ticker: &str,
            _name: Option<&str>,
            _question: &str,
        ) -> Vec<Evidence> {
            self.evidence_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.evidence.clone()
        }

        async fn load_prior_turns(&self, _user_id: &str, _session_id: &str) -> Vec<PriorTurn> {
            self.prior_turns.clone()
        }

        async fn complete_answer(
            &self,
            _system: &str,
            _user: &str,
            _user_id: &str,
        ) -> Option<String> {
            self.answer.clone()
        }

        async fn stream_answer(
            &self,
            _system: String,
            _user: String,
            _user_id: String,
        ) -> ModelStreamStart {
            if self.stream_unavailable {
                return ModelStreamStart::Unavailable;
            }
            let (tx, rx) = mpsc::channel(8);
            let chunks = self.stream_chunks.clone();
            let fail = self.stream_fail.clone();
            tokio::spawn(async move {
                for chunk in chunks {
                    let _ = tx.send(ModelStreamEvent::Delta(chunk)).await;
                }
                if let Some(message) = fail {
                    let _ = tx.send(ModelStreamEvent::Failed { message }).await;
                } else {
                    let _ = tx.send(ModelStreamEvent::Completed).await;
                }
            });
            ModelStreamStart::Ready(rx)
        }

        async fn save_session(
            &self,
            _user_id: &str,
            session: PersistResearchSession,
        ) -> Result<String, String> {
            if self.fail_save {
                return Err("db down".into());
            }
            let id = session
                .id
                .clone()
                .unwrap_or_else(|| format!("s_{}", self.saved.lock().expect("lock").len()));
            self.saved.lock().expect("lock").push(session);
            Ok(id)
        }
    }

    async fn collect_stream(
        mut rx: mpsc::Receiver<ResearchStreamEvent>,
    ) -> Vec<ResearchStreamEvent> {
        let mut out = Vec::new();
        while let Some(event) = rx.recv().await {
            out.push(event);
        }
        out
    }

    #[tokio::test]
    async fn request_facts_win_over_empty_ports() {
        let ports = FakePorts::default();
        let req = AskRequest {
            question: "估值怎么样".into(),
            ticker: "AAPL".into(),
            price: Some(dec!(190)),
            pe: Some(dec!(28)),
            draft_answer: Some("现价约 190 美元。".into()),
            ..Default::default()
        };
        let outcome = ResearchService::ask(&ports, "user-1", req).await;
        assert_eq!(outcome.facts.market.price, Some(dec!(190)));
        assert_eq!(outcome.response.answer_source, AnswerSource::Draft);
        assert!(outcome.response.fact_guard.is_some());
        assert!(outcome.persisted);
        assert_eq!(outcome.route.intent.as_str(), "valuation");
    }

    #[tokio::test]
    async fn continuing_session_reuses_id_and_feeds_history_not_facts() {
        let ports = FakePorts {
            prior_turns: vec![PriorTurn {
                question: "苹果的护城河是什么？".into(),
                answer: "生态锁定与服务收入占比提升。".into(),
            }],
            answer: Some("现价约 190 美元，估值偏贵。".into()),
            ..FakePorts::default()
        };
        let req = AskRequest {
            question: "它的估值贵不贵？".into(),
            ticker: "AAPL".into(),
            session_id: Some("s_existing".into()),
            ..Default::default()
        };
        let outcome = ResearchService::ask(&ports, "user-1", req).await;
        assert_eq!(outcome.response.session_id.as_deref(), Some("s_existing"));
        let saved = ports.saved.lock().expect("lock");
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].id.as_deref(), Some("s_existing"));
        assert_eq!(saved[0].turn_count, Some(2), "此前一轮 + 本轮");
        let thread: Vec<PriorTurn> =
            serde_json::from_value(saved[0].thread.clone().expect("thread")).expect("decode");
        assert_eq!(thread.len(), 2);
        assert_eq!(thread[0].question, "苹果的护城河是什么？");
        assert_eq!(thread[1].question, "它的估值贵不贵？");
    }

    #[tokio::test]
    async fn save_failure_on_continuation_still_returns_known_session_id() {
        let ports = FakePorts {
            fail_save: true,
            answer: Some("ok".into()),
            ..FakePorts::default()
        };
        let req = AskRequest {
            question: "还有其他风险吗".into(),
            ticker: "AAPL".into(),
            session_id: Some("s_existing".into()),
            ..Default::default()
        };
        let outcome = ResearchService::ask(&ports, "user-1", req).await;
        assert!(!outcome.persisted);
        assert_eq!(outcome.response.session_id.as_deref(), Some("s_existing"));
    }

    #[tokio::test]
    async fn compare_keeps_two_legs_isolated_and_guards_each_separately() {
        let mut market_by_ticker = std::collections::HashMap::new();
        market_by_ticker.insert(
            "AAPL".to_string(),
            (
                ResolvedCompany {
                    ticker: "AAPL".into(),
                    name_zh: Some("苹果".into()),
                    company: Company {
                        price: Some(dec!(190)),
                        ..Default::default()
                    },
                },
                MarketSnapshot {
                    price: Some(dec!(190)),
                    currency: Some("USD".into()),
                    ..Default::default()
                },
            ),
        );
        market_by_ticker.insert(
            "0700.HK".to_string(),
            (
                ResolvedCompany {
                    ticker: "0700.HK".into(),
                    name_zh: Some("腾讯".into()),
                    company: Company {
                        price: Some(dec!(300)),
                        ..Default::default()
                    },
                },
                MarketSnapshot {
                    price: Some(dec!(300)),
                    currency: Some("HKD".into()),
                    ..Default::default()
                },
            ),
        );
        let mut fundamentals_by_ticker = std::collections::HashMap::new();
        fundamentals_by_ticker.insert(
            "AAPL".to_string(),
            LoadedFundamentals {
                financials: Financials {
                    provider_ok: true,
                    revenue: Some(dec!(383000)),
                    currency: Some("USD".into()),
                    ..Default::default()
                },
                pe_ttm: None,
                company_name: Some("苹果".into()),
            },
        );
        fundamentals_by_ticker.insert(
            "0700.HK".to_string(),
            LoadedFundamentals {
                financials: Financials {
                    provider_ok: true,
                    revenue: Some(dec!(160000)),
                    currency: Some("HKD".into()),
                    ..Default::default()
                },
                pe_ttm: None,
                company_name: Some("腾讯".into()),
            },
        );
        let ports = FakePorts {
            market_by_ticker,
            fundamentals_by_ticker,
            answer: Some(
                "苹果营收约383000美元。这段说明性文字用来把两家公司的数字隔开一段距离。\
                 腾讯营收约160000港元。两者护城河判断见 [A1] 与 [B1]。"
                    .into(),
            ),
            evidence: vec![Evidence {
                title: "独立公司来源".into(),
                url: "https://example.com/source".into(),
                snippet: "定性证据".into(),
                ..Default::default()
            }],
            ..FakePorts::default()
        };

        let outcome = ResearchService::compare(
            &ports,
            "user-1",
            "谁的护城河更强？".into(),
            "AAPL".into(),
            "0700.HK".into(),
        )
        .await;

        assert_eq!(outcome.facts.primary.company.ticker, "AAPL");
        assert_eq!(outcome.facts.peer.company.ticker, "0700.HK");
        assert_eq!(outcome.response.primary.ticker, "AAPL");
        assert_eq!(outcome.response.peer.ticker, "0700.HK");
        assert_eq!(outcome.response.primary.sources.len(), 1);
        assert_eq!(outcome.response.peer.sources.len(), 1);
        assert_eq!(
            ports
                .evidence_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            2,
            "对比两腿应各自检索一次，不能只给主腿取证"
        );
        // 两腿的营收数字都在各自事实块里真实存在，护栏各自应给 pass（不合并、不互相污染）。
        let primary_guard = outcome.response.primary.fact_guard.expect("primary guard");
        let peer_guard = outcome.response.peer.fact_guard.expect("peer guard");
        assert!(primary_guard.pass >= 1, "苹果自己的营收应命中自己的登记表");
        assert!(peer_guard.pass >= 1, "腾讯自己的营收应命中自己的登记表");
    }

    #[tokio::test]
    async fn qualitative_intent_pulls_evidence_numeric_intent_does_not() {
        let evidence = vec![Evidence {
            title: "Apple widens services moat".into(),
            url: "https://reuters.com/x".into(),
            snippet: "生态锁定与服务收入占比继续提升。".into(),
            published_date: Some("2026-07-01".into()),
            source_domain: Some("reuters.com".into()),
        }];
        // 护城河（定性意图）→ 拉证据并进 response.sources。
        let ports = FakePorts {
            answer: Some("护城河主要在生态锁定。".into()),
            evidence: evidence.clone(),
            ..FakePorts::default()
        };
        let outcome =
            ResearchService::ask(&ports, "u", AskRequest::minimal("它的护城河在哪", "AAPL")).await;
        assert_eq!(outcome.facts.evidence.len(), 1);
        assert_eq!(outcome.response.sources.len(), 1);
        assert_eq!(
            outcome.response.sources[0].source_domain.as_deref(),
            Some("reuters.com")
        );
        // 有证据但答案没标来源 → 引用护栏在场且判裸奔（soft 提示，非 hard）。
        let cg = outcome.response.citation_guard.expect("citation guard");
        assert_eq!(cg.evidence_count, 1);
        assert_eq!(cg.cited_count, 0);
        assert!(cg.ungrounded);
        assert!(!cg.has_hard_fail);

        // 估值（数字驱动意图）→ 门控关闭，即便端口有证据也不拉、不进 sources、无引用护栏。
        let ports = FakePorts {
            answer: Some("PE 约 28 倍。".into()),
            evidence,
            ..FakePorts::default()
        };
        let outcome =
            ResearchService::ask(&ports, "u", AskRequest::minimal("现在估值贵不贵", "AAPL")).await;
        assert!(outcome.facts.evidence.is_empty());
        assert!(outcome.response.sources.is_empty());
        assert!(outcome.response.citation_guard.is_none());
    }

    #[tokio::test]
    async fn deep_depth_runs_multi_query_evidence_and_dedups() {
        use std::sync::atomic::Ordering;
        let ev = vec![
            Evidence {
                url: "https://a.com/1".into(),
                ..Default::default()
            },
            Evidence {
                url: "https://b.com/2".into(),
                ..Default::default()
            },
        ];
        let ports = FakePorts {
            evidence: ev,
            ..FakePorts::default()
        };
        // deep：基础问题 + 3 个维度 = 4 次并发检索；各次返回相同两条 → 按 URL 去重后仍 2 条。
        let merged = gather_evidence(
            &ports,
            "AAPL",
            Some("苹果"),
            "深度研究苹果",
            ResearchDepth::Deep,
        )
        .await;
        assert_eq!(merged.len(), 2, "跨查询按 URL 去重");
        assert_eq!(
            ports.evidence_calls.load(Ordering::SeqCst),
            4,
            "基础问题 + 3 个维度"
        );

        // standard：单条查询，不铺开。
        let ports2 = FakePorts {
            evidence: vec![Evidence {
                url: "https://a.com/1".into(),
                ..Default::default()
            }],
            ..FakePorts::default()
        };
        let _ = gather_evidence(&ports2, "AAPL", None, "苹果怎么样", ResearchDepth::Standard).await;
        assert_eq!(ports2.evidence_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn citation_guard_flags_hallucinated_source_number() {
        let evidence = vec![Evidence {
            title: "Apple moat".into(),
            url: "https://reuters.com/x".into(),
            snippet: "生态锁定。".into(),
            published_date: None,
            source_domain: Some("reuters.com".into()),
        }];
        // 只有 1 条证据，答案却引用了 [3] → 虚构来源号，引用护栏应判 hard。
        let ports = FakePorts {
            answer: Some("护城河见来源[1]，风险见来源[3]。".into()),
            evidence,
            ..FakePorts::default()
        };
        let outcome =
            ResearchService::ask(&ports, "u", AskRequest::minimal("护城河与风险", "AAPL")).await;
        let cg = outcome.response.citation_guard.expect("citation guard");
        assert_eq!(cg.cited_count, 1);
        assert_eq!(cg.out_of_range, 1);
        assert!(cg.has_hard_fail);
    }

    #[tokio::test]
    async fn unavailable_model_still_returns_structured_panel() {
        let ports = FakePorts {
            answer: None,
            ..FakePorts::default()
        };
        let req = AskRequest::minimal("护城河？", "0700.HK");
        let outcome = ResearchService::ask(&ports, "user-1", req).await;
        assert_eq!(outcome.response.answer_source, AnswerSource::Unavailable);
        assert!(outcome.response.answer.is_none());
        assert_eq!(outcome.response.ticker, "0700.HK");
    }

    #[tokio::test]
    async fn save_failure_does_not_drop_response() {
        let ports = FakePorts {
            fail_save: true,
            answer: Some("ok".into()),
            ..FakePorts::default()
        };
        let req = AskRequest::minimal("怎么赚钱", "0700.HK");
        let outcome = ResearchService::ask(&ports, "user-1", req).await;
        assert!(!outcome.persisted);
        assert_eq!(outcome.response.answer.as_deref(), Some("ok"));
    }

    #[tokio::test]
    async fn db_fill_used_when_request_has_no_price() {
        let ports = FakePorts {
            market: Some((
                ResolvedCompany {
                    ticker: "NVDA".into(),
                    name_zh: Some("英伟达".into()),
                    company: Company {
                        price: Some(dec!(120)),
                        ..Default::default()
                    },
                },
                MarketSnapshot {
                    price: Some(dec!(120)),
                    currency: Some("USD".into()),
                    ..Default::default()
                },
            )),
            answer: Some("现价约 120。".into()),
            ..FakePorts::default()
        };
        let req = AskRequest::minimal("股价？", "NVDA");
        let outcome = ResearchService::ask(&ports, "u", req).await;
        assert_eq!(outcome.facts.market.price, Some(dec!(120)));
        assert_eq!(outcome.facts.company.name_zh.as_deref(), Some("英伟达"));
        let saved = ports.saved.lock().expect("lock");
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].company_name.as_deref(), Some("英伟达"));
    }

    #[tokio::test]
    async fn fundamentals_port_fills_when_request_has_no_financials() {
        let ports = FakePorts {
            fundamentals: Some(LoadedFundamentals {
                financials: Financials {
                    provider_ok: true,
                    revenue: Some(dec!(100)),
                    eps: Some(dec!(1.5)),
                    eps_annualized: Some(false),
                    ..Default::default()
                },
                pe_ttm: Some(dec!(28)),
                company_name: Some("Apple Inc.".into()),
            }),
            answer: Some("ok".into()),
            ..FakePorts::default()
        };
        let req = AskRequest {
            question: "估值".into(),
            ticker: "AAPL".into(),
            price: Some(dec!(190)),
            ..Default::default()
        };
        let facts = ResearchService::assemble_facts(&ports, &req).await;
        assert!(facts.financials.provider_ok);
        assert_eq!(facts.financials.revenue, Some(dec!(100)));
        assert_eq!(facts.financials.eps_annualized, Some(false));
        assert_eq!(facts.market.pe, Some(dec!(28)));
        assert_eq!(facts.company.name_zh.as_deref(), Some("Apple Inc."));
    }

    #[tokio::test]
    async fn request_financials_win_over_fundamentals_port() {
        let ports = FakePorts {
            fundamentals: Some(LoadedFundamentals {
                financials: Financials {
                    provider_ok: true,
                    revenue: Some(dec!(1)),
                    ..Default::default()
                },
                pe_ttm: Some(dec!(99)),
                company_name: Some("ignored".into()),
            }),
            ..FakePorts::default()
        };
        let req = AskRequest {
            question: "估值".into(),
            ticker: "AAPL".into(),
            revenue: Some(dec!(50)),
            pe: Some(dec!(20)),
            ..Default::default()
        };
        let facts = ResearchService::assemble_facts(&ports, &req).await;
        assert_eq!(facts.financials.revenue, Some(dec!(50)));
        assert_eq!(facts.market.pe, Some(dec!(20)));
        assert!(facts.company.name_zh.is_none());
    }

    #[tokio::test]
    async fn typed_stream_emits_guard_and_final_after_deltas() {
        let ports = FakePorts {
            stream_chunks: vec!["现价约 ".into(), "100。".into()],
            ..FakePorts::default()
        };
        let req = AskRequest::minimal("股价怎么样", "AAPL");
        let events = collect_stream(ResearchService::ask_stream(ports, "u1".into(), req)).await;
        let names: Vec<_> = events.iter().map(ResearchStreamEvent::event_name).collect();
        assert_eq!(
            names,
            vec![
                "stage", "stage", "stage", "stage", "stage", "meta", "stage", "delta", "delta",
                "stage", "guard", "final"
            ]
        );
        let stages = events
            .iter()
            .filter_map(|event| match event {
                ResearchStreamEvent::Stage(stage) => Some(stage),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            stages.iter().map(|stage| stage.name).collect::<Vec<_>>(),
            vec![
                ResearchStreamStageName::Routing,
                ResearchStreamStageName::Resolving,
                ResearchStreamStageName::MarketFinancials,
                ResearchStreamStageName::Evidence,
                ResearchStreamStageName::Valuation,
                ResearchStreamStageName::Generating,
                ResearchStreamStageName::FactCheck,
            ]
        );
        assert!(
            stages
                .iter()
                .enumerate()
                .all(|(index, stage)| stage.index == index + 1 && stage.total == stages.len()),
            "阶段序号必须与 route.plan 一一对齐"
        );
        let ResearchStreamEvent::Final(final_event) = events.last().expect("final") else {
            panic!("expected final");
        };
        assert_eq!(final_event.response.answer.as_deref(), Some("现价约 100。"));
        assert!(final_event.persisted);
        assert_eq!(final_event.response.answer_source, AnswerSource::Generated);
    }

    #[tokio::test]
    async fn typed_stream_failure_skips_persist() {
        let ports = FakePorts {
            stream_chunks: vec!["半句".into()],
            stream_fail: Some("provider down".into()),
            ..FakePorts::default()
        };
        let req = AskRequest::minimal("护城河", "0700.HK");
        let events = collect_stream(ResearchService::ask_stream(ports, "u1".into(), req)).await;
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ResearchStreamEvent::Error(_)))
        );
        assert!(
            events
                .iter()
                .all(|e| !matches!(e, ResearchStreamEvent::Final(_)))
        );
    }

    #[tokio::test]
    async fn typed_stream_save_failure_still_finalizes() {
        let ports = FakePorts {
            stream_chunks: vec!["全文".into()],
            fail_save: true,
            ..FakePorts::default()
        };
        let req = AskRequest::minimal("商业模式", "0700.HK");
        let events = collect_stream(ResearchService::ask_stream(ports, "u1".into(), req)).await;
        let ResearchStreamEvent::Final(final_event) = events.last().expect("final") else {
            panic!("expected final");
        };
        assert!(!final_event.persisted);
        assert_eq!(final_event.response.answer.as_deref(), Some("全文"));
    }
}
