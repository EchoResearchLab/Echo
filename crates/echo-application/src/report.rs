//! 深度报告用例——判断优先的长文 Markdown，与 `/api/ask` 共用同一条取数/估值/护栏管线
//! （`ResearchService::assemble_facts` + `build_panel`），只在提示词与产物形态上分叉：
//! 报告不是聊天回答的重命名，固定结构、更长篇幅，且必须能在模型不可用时退化为本地
//! 确定性报告——两条路径都只引用同一份 `FactsRegistry` 已核数字。

use crate::answer_prompt::{
    AnswerContext, build_system_prompt, facts_block, field, supplemental_research_context,
    valuation_line,
};
use crate::research::{
    PersistResearchSession, PriorTurn, ResearchFacts, ResearchPorts, ResearchService,
    citation_guard_view, earnings_view, filings_view, guard_outcome, load_prior_turns_for,
    persist_company_memory_if_safe, record_guard_audit, route_view, valuation_view,
};
use crate::research_memory::CompanyMemory;
use crate::{DecisionPanel, build_panel};
use echo_contracts::{AskRequest, ReportGenerateResponse, ReportMode};
use echo_domain::ResearchRoute;

const DISCLAIMER: &str =
    "\n\n---\n> 本报告仅供研究学习，不构成投资建议。请用公司原始公告核验关键数据，独立做出决定。\n";

/// 模型输出短于这个长度视为不可用（截断/拒答），转本地兜底——与老 JS 版本口径一致。
const MIN_MODEL_REPORT_CHARS: usize = 200;

/// 一次深度报告生成的结果；可追溯主体、护栏与是否已落库。
#[derive(Clone, Debug)]
pub struct ReportOutcome {
    pub response: ReportGenerateResponse,
    pub facts: ResearchFacts,
    pub route: ResearchRoute,
    pub persisted: bool,
}

#[derive(Clone, Debug)]
pub struct ReportService;

impl ReportService {
    /// 深度报告主用例：同一条单公司取数管线，报告专属提示词，模型不可用/输出过短时退化
    /// 为本地确定性报告，护栏对最终产出的 markdown 核数，落库归位同一研究会话。
    pub async fn generate<P: ResearchPorts>(
        ports: &P,
        user_id: &str,
        req: AskRequest,
    ) -> ReportOutcome {
        let route = echo_domain::route_research_intent(&req.question);
        let facts = ResearchService::assemble_facts(ports, &req).await;
        let panel = build_panel(
            &facts.company,
            &facts.market,
            &facts.financials,
            facts.peer_anchor.as_ref(),
            &facts.filings,
        );
        let (prior_turns, memory) = tokio::join!(
            load_prior_turns_for(ports, user_id, &req),
            ports.load_company_memory(user_id, &req.ticker)
        );
        let ctx = AnswerContext {
            question: &req.question,
            name_zh: facts.company.name_zh.as_deref(),
            panel: &panel,
            market: &facts.market,
            financials: &facts.financials,
            filings: &facts.filings,
            evidence: &facts.evidence,
            depth: route.depth,
            history: &prior_turns,
        };

        let system = build_report_system_prompt();
        let user_prompt = build_report_prompt(
            &req.question,
            &ctx,
            memory.as_ref(),
            facts.peer_anchor.as_ref(),
            facts.recovery.degradation,
        );
        let model_answer = ports.complete_answer(&system, &user_prompt, user_id).await;
        let (markdown, mode) = match model_answer {
            Some(text) if text.trim().chars().count() > MIN_MODEL_REPORT_CHARS => {
                (format!("{}{DISCLAIMER}", text.trim()), ReportMode::Model)
            }
            _ => (compose_report_fallback(&ctx), ReportMode::Local),
        };

        let outcome = guard_outcome(&req, &facts, &panel, &markdown);
        record_guard_audit(ports, &req.ticker, "report", outcome.clone()).await;
        let fact_guard = Some(outcome.view);
        let citation_guard = citation_guard_view(&facts.evidence, &markdown);
        persist_company_memory_if_safe(
            ports,
            user_id,
            &req,
            &facts,
            &panel,
            memory.as_ref(),
            Some(&markdown),
            fact_guard.as_ref(),
            citation_guard.as_ref(),
        )
        .await;

        let response = ReportGenerateResponse {
            ticker: panel.ticker.clone(),
            route: route_view(&route),
            mode,
            markdown: markdown.clone(),
            valuation: valuation_view(&panel),
            fact_guard,
            citation_guard,
            earnings: earnings_view(facts.earnings_calendar.as_ref()),
            filings: filings_view(&facts.filings),
            session_id: None,
        };
        let (persisted, session_id) = persist_report(
            ports,
            user_id,
            &req,
            &facts,
            &panel,
            &markdown,
            &prior_turns,
        )
        .await;
        let mut response = response;
        response.session_id = session_id;

        ReportOutcome {
            response,
            facts,
            route,
            persisted,
        }
    }
}

/// system 红线沿用聊天回答的一套，加一句报告体裁与人设的补充——不是重新发明纪律。
fn build_report_system_prompt() -> String {
    format!(
        "{}\n· 本次任务是写一篇结构化深度研究报告（不是聊天回答），资深买方研究员风格、\n  判断优先、克制、可证伪，绝不暴露后台/产品/厂商词，绝不给买卖指令。",
        build_system_prompt()
    )
}

/// 报告 user 提示词：与聊天回答共用同一份事实块（`facts_block`），只在结构与篇幅要求上
/// 分叉——报告固定七段、1200-2500 字，聊天回答不强制结构。
fn build_report_prompt(
    question: &str,
    ctx: &AnswerContext,
    memory: Option<&CompanyMemory>,
    peer_anchor: Option<&echo_domain::PeerAnchor>,
    degradation: crate::research_orchestrator::ResearchDegradation,
) -> String {
    let name = ctx.name_zh.unwrap_or(ctx.panel.ticker.as_str());
    let question = if question.trim().is_empty() {
        format!("{name} 值不值得研究")
    } else {
        question.to_string()
    };
    let mut out = String::new();
    out.push_str(&format!(
        "请基于以下已核事实，为 {name}（{}）写一份资深买方研究员风格的深度研究报告。\n用户问题：{question}\n\n",
        ctx.panel.ticker
    ));
    out.push_str(&facts_block(ctx));
    out.push_str(&supplemental_research_context(
        memory,
        peer_anchor,
        degradation,
    ));
    out.push_str(
        "\n\n写作规则：\n\
         - 输出中文 Markdown，用 ## 小标题分节，不要表格。\n\
         - 固定结构，依次：## 核心判断、## 赚钱机制与护城河、## 财务质量、## 估值与赔率、\n\
           ## 风险与证伪条件、## 关键监控与下一步、## 来源。\n\
         - 判断优先：开头第一段直接给核心判断——它现在赚不赚钱、质量如何、最大的赌点和\n\
           最大的风险各是什么，不要用「我将分析/数据不足」开场。\n\
         - 财务与估值数字只能引用上面「已核到」的事实块；缺某项就用研究语言说「当前未核到」，\n\
           仍要给出方向性判断，不得编造。\n\
         - 「## 来源」只列上面出现过的连通数据源与公司公告，不要编造链接。\n\
         - 不给买入/卖出/持有指令，用观察、验证、赔率改善、逻辑重估等研究语言，结尾不需要\n\
           再写免责声明（系统会附加）。\n\
         - 长度 1200-2500 字，信息密度优先，不要空话。",
    );
    out
}

/// 模型不可用/输出过短时的本地确定性报告——只用 `ctx` 里已核到的数字拼接，不发明业务
/// 定性描述（Rust 侧目前没有 company_profiles 自动回填到研究管线，属 pending 能力），
/// 比模型报告更朴素，但绝不编数字。
fn compose_report_fallback(ctx: &AnswerContext) -> String {
    let name = ctx.name_zh.unwrap_or(&ctx.panel.ticker);
    let panel = ctx.panel;
    let mut out = String::new();

    out.push_str(&format!("# {name}（{}）深度研究\n\n", panel.ticker));
    out.push_str(&format!(
        "> 数据完整度 {}% · 连通数据源：{}\n\n",
        panel.data_completeness,
        if panel.connected_sources.is_empty() {
            "无（本轮多为定性判断）".to_string()
        } else {
            panel.connected_sources.join("、")
        }
    ));

    out.push_str("## 核心判断\n\n");
    out.push_str(&format!(
        "{name} 现价 {}。{}\n\n",
        field(ctx.market.price, ""),
        valuation_line(panel)
    ));

    out.push_str("## 赚钱机制与护城河\n\n");
    out.push_str(
        "核心收入来源与护城河需要结合最新财报与公告拆分；判断它是否真的赚钱，先看高毛利\n业务占比是否提升、利润率是否稳定、现金流能否同向兑现。\n\n",
    );

    out.push_str("## 财务质量\n\n");
    if ctx.financials.provider_ok {
        let cur = ctx.financials.currency.as_deref().unwrap_or("");
        out.push_str(&format!("- 营收：{}\n", field(ctx.financials.revenue, cur)));
        out.push_str(&format!(
            "- 净利润：{}\n",
            field(ctx.financials.net_income, cur)
        ));
        out.push_str(&format!(
            "- 净利率：{}\n",
            field(ctx.financials.net_margin, "%")
        ));
        out.push_str(&format!(
            "- 毛利率：{}\n",
            field(ctx.financials.gross_margin, "%")
        ));
        out.push_str(&format!(
            "- 自由现金流：{}\n\n",
            field(ctx.financials.free_cash_flow, cur)
        ));
    } else {
        out.push_str("本轮无实时财报，暂未核到具体数字，需补数后再做财务判断。\n\n");
    }

    out.push_str("## 估值与赔率\n\n");
    out.push_str(&format!("{}\n\n", valuation_line(panel)));

    out.push_str("## 风险与证伪条件\n\n");
    out.push_str(
        "需持续跟踪竞争格局、监管环境与现金流趋势；一旦营收、利润率或自由现金流方向性走弱，\n应按逻辑重估当前判断，而不是用「便宜/被低估」自我安慰。\n\n",
    );

    out.push_str("## 关键监控与下一步\n\n");
    out.push_str(
        "持续跟踪营收增速、利润率、自由现金流与股东回报节奏，作为先行指标盯趋势，而不是等\n财报盖棺；完整财报三表与最新公告核到后，可把当前方向性判断升级为区间判断。\n\n",
    );

    out.push_str("## 来源\n\n");
    if ctx.filings.is_empty() && panel.connected_sources.is_empty() {
        out.push_str("- 暂无已核到的公开来源\n");
    } else {
        if !panel.connected_sources.is_empty() {
            out.push_str(&format!(
                "- 数据源：{}\n",
                panel.connected_sources.join("、")
            ));
        }
        for filing in ctx.filings {
            out.push_str(&format!(
                "- {} · {} · {}\n",
                filing.form,
                filing.filed_date.as_deref().unwrap_or("未核到日期"),
                filing.source_url
            ));
        }
    }

    out.push_str(DISCLAIMER);
    out
}

/// 落库并返回 `(是否成功, 归位的会话 id)`——与 `research.rs::persist_outcome` 同一套收口
/// 逻辑，区别只在报告同时写 `report_markdown` 与 `full_research`（两者同源同一份 markdown）。
async fn persist_report<P: ResearchPorts>(
    ports: &P,
    user_id: &str,
    req: &AskRequest,
    facts: &ResearchFacts,
    panel: &DecisionPanel,
    markdown: &str,
    prior_turns: &[PriorTurn],
) -> (bool, Option<String>) {
    let mut entries = prior_turns.to_vec();
    entries.push(PriorTurn {
        question: req.question.clone(),
        answer: markdown.to_string(),
    });
    let thread = serde_json::to_value(&entries).ok();
    let session = PersistResearchSession {
        id: req.session_id.clone(),
        ticker: req.ticker.clone(),
        company_name: facts.company.name_zh.clone(),
        question: Some(req.question.clone()),
        report_markdown: Some(markdown.to_string()),
        decision_panel: serde_json::to_value(valuation_view(panel)).ok(),
        full_research: Some(markdown.to_string()),
        data_sources: Some(serde_json::json!({
            "connected": panel.connected_sources.clone(),
            "recovery": facts.recovery,
        })),
        turn_count: Some(prior_turns.len() as i32 + 1),
        thread,
    };
    match ports.save_session(user_id, session).await {
        Ok(id) => (true, Some(id)),
        Err(_) => (false, req.session_id.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_gateway::ModelStreamStart;
    use crate::research::LoadedFundamentals;
    use echo_domain::{
        Company, EarningsCalendar, Evidence, Filing, Financials, HistoricalValuation,
        MarketSnapshot, MultipleType, PeerAnchor,
    };
    use rust_decimal_macros::dec;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakePorts {
        market: Option<(crate::ResolvedCompany, MarketSnapshot)>,
        financials: Option<Financials>,
        answer: Option<String>,
        saved: Mutex<Vec<PersistResearchSession>>,
    }

    impl ResearchPorts for FakePorts {
        async fn load_company_market(
            &self,
            _ticker: &str,
        ) -> Option<(crate::ResolvedCompany, MarketSnapshot)> {
            self.market.clone()
        }

        async fn refresh_quote(&self, _ticker: &str) -> Result<(), String> {
            Err("unavailable".into())
        }

        async fn load_fundamentals(&self, _ticker: &str) -> Option<LoadedFundamentals> {
            self.financials
                .clone()
                .map(|financials| LoadedFundamentals {
                    financials,
                    pe_ttm: None,
                    company_name: None,
                })
        }

        async fn load_earnings_calendar(&self, _ticker: &str) -> Option<EarningsCalendar> {
            None
        }

        async fn load_historical_valuation(&self, _ticker: &str) -> Option<HistoricalValuation> {
            None
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
            Vec::new()
        }

        async fn load_prior_turns(&self, _user_id: &str, _session_id: &str) -> Vec<PriorTurn> {
            Vec::new()
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
            ModelStreamStart::Unavailable
        }

        async fn save_session(
            &self,
            _user_id: &str,
            session: PersistResearchSession,
        ) -> Result<String, String> {
            let id = session
                .id
                .clone()
                .unwrap_or_else(|| format!("s_{}", self.saved.lock().expect("lock").len()));
            self.saved.lock().expect("lock").push(session);
            Ok(id)
        }
    }

    fn market_for(
        ticker: &str,
        price: rust_decimal::Decimal,
    ) -> (crate::ResolvedCompany, MarketSnapshot) {
        (
            crate::ResolvedCompany {
                ticker: ticker.into(),
                name_zh: Some("苹果".into()),
                company: Company {
                    price: Some(price),
                    ..Default::default()
                },
            },
            MarketSnapshot {
                price: Some(price),
                currency: Some("USD".into()),
                ..Default::default()
            },
        )
    }

    #[tokio::test]
    async fn falls_back_to_local_report_when_model_unavailable() {
        let ports = FakePorts {
            market: Some(market_for("AAPL", dec!(190))),
            financials: Some(Financials {
                provider_ok: true,
                revenue: Some(dec!(383000)),
                currency: Some("USD".into()),
                ..Default::default()
            }),
            answer: None,
            ..FakePorts::default()
        };
        let req = AskRequest::minimal("值不值得研究", "AAPL");
        let outcome = ReportService::generate(&ports, "user-1", req).await;
        assert_eq!(outcome.response.mode, ReportMode::Local);
        assert!(
            outcome
                .response
                .markdown
                .starts_with("# 苹果（AAPL）深度研究")
        );
        assert!(outcome.response.markdown.contains("383000"));
        assert!(outcome.persisted);
        let saved = ports.saved.lock().expect("lock");
        assert_eq!(saved.len(), 1);
        assert!(saved[0].report_markdown.is_some());
    }

    #[tokio::test]
    async fn short_model_output_also_falls_back_to_local() {
        let ports = FakePorts {
            market: Some(market_for("AAPL", dec!(190))),
            answer: Some("太短".into()),
            ..FakePorts::default()
        };
        let req = AskRequest::minimal("怎么样", "AAPL");
        let outcome = ReportService::generate(&ports, "user-1", req).await;
        assert_eq!(outcome.response.mode, ReportMode::Local);
    }

    #[tokio::test]
    async fn long_model_output_is_used_and_gets_disclaimer() {
        let long_answer = "核心判断：".to_string() + &"业务分析。".repeat(60);
        let ports = FakePorts {
            market: Some(market_for("AAPL", dec!(190))),
            answer: Some(long_answer.clone()),
            ..FakePorts::default()
        };
        let req = AskRequest::minimal("深度研究一下", "AAPL");
        let outcome = ReportService::generate(&ports, "user-1", req).await;
        assert_eq!(outcome.response.mode, ReportMode::Model);
        assert!(outcome.response.markdown.starts_with(&long_answer));
        assert!(outcome.response.markdown.contains("不构成投资建议"));
    }
}
