//! 作答提示词构造——从领域事实拼出 system 红线 + user 事实块，喂给模型网关生成答案。
//!
//! 作答纪律集中在这里：不编数字、缺就说「未核到」、无实时财报禁给具体财务数字、不提买卖建议。
//! 同业对照、网页证据、回购与对比研究都通过结构化事实块显式接入。
//!
//! 全是纯函数（事实进、字符串出），不碰网络/时钟，可离线单测每条红线分支。

use crate::DecisionPanel;
use crate::research::PriorTurn;
use crate::research_memory::{CompanyMemory, memory_prompt_block};
use crate::research_orchestrator::ResearchDegradation;
use echo_domain::{
    Evidence, Filing, Financials, MarketSnapshot, MultipleType, PeerAnchor, ResearchDepth,
};
use rust_decimal::Decimal;

/// 作答上下文——本请求这一家公司的全部已核事实（单一主体，无跨公司泄漏面）。
pub struct AnswerContext<'a> {
    pub question: &'a str,
    pub name_zh: Option<&'a str>,
    pub panel: &'a DecisionPanel,
    pub market: &'a MarketSnapshot,
    pub financials: &'a Financials,
    pub filings: &'a [Filing],
    /// 网页证据（定性意图专属）——供模型定性引用并标注来源；二手信息，不得当作已核财务数字。
    pub evidence: &'a [Evidence],
    /// 研究深度——决定作答风格（brief 直接精简 / deep 分维度系统展开）。
    pub depth: ResearchDepth,
    /// 同一研究会话此前几轮的问答——只帮模型承接代词/实体指代，不得当作本轮数字来源。
    pub history: &'a [PriorTurn],
}

/// 深度对应的作答风格指令——让 depth 真正改变产出形态，不只是路由标签。
fn depth_directive(depth: ResearchDepth) -> &'static str {
    match depth {
        ResearchDepth::Brief => "\n作答风格：直接精简——一两句给出结论与关键理由，不逐条展开。\n",
        ResearchDepth::Deep => {
            "\n作答风格：系统展开——分维度（赚钱机制 / 护城河 / 财务质量 / 风险与证伪 / 近况）\
             逐条给判断，充分利用上方网页证据并逐条标注来源编号，最后给一句总判断。\n"
        }
        ResearchDepth::Standard => "",
    }
}

/// 固定的 system 红线——研究助手人设 + 不可逾越的护栏。与 composer 的 system 段同义：
/// 不构成投资建议、不提买卖、缺数说「未核到」、财务数字只能引用已核到的实时财报、不编造。
#[must_use]
pub fn build_system_prompt() -> String {
    [
        "你是严谨的股票研究助手，只做美股与港股科技股的基本面研究。铁律：",
        "· 以下内容仅供分析参考，不构成投资建议，绝不给出任何买入/卖出/加减仓指令。",
        "· 凡收入/利润/利润率/现金流/EPS/回购分红的具体数字，只能引用下面「已核到的实时财报」块；本地印象/常识一律不得当作财务数字来源，标注「约」「大约」「行业常见范围」也不行。",
        "· 任一数据缺失就明说「当前未核到/来源缺失」，置信度下降，但可继续给定性推断。",
        "· 不用「暂不评分」「完整度xx%」这类产品状态词，改用研究语言（未核到、置信度下降）。",
    ]
    .join("\n")
}

/// 按字符边界截断（多字节安全），超长追加省略号提示这是摘要而非全文。
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

/// 把一个可选定点数渲染成事实行的值；缺就「未核到」。
pub(crate) fn field(value: Option<Decimal>, unit: &str) -> String {
    match value {
        Some(v) => format!("{v}{unit}"),
        None => "未核到".to_string(),
    }
}

/// 估值区间事实行——给不出可信带子（`cannot_value_reason`）时如实说未核到，绝不硬凑数字。
pub(crate) fn valuation_line(panel: &DecisionPanel) -> String {
    let v = &panel.valuation;
    if !v.is_valued() {
        let reason = v.cannot_value_reason.as_deref().unwrap_or("数据不足");
        return format!("估值区间：未核到（{reason}）");
    }
    let bear = field(v.bear, "");
    let base = field(v.base, "");
    let bull = field(v.bull, "");
    let upside = v.upside.as_deref().unwrap_or("未核到");
    format!(
        "估值区间（{}）：熊 {bear} / 基准 {base} / 牛 {bull}；相对现价空间 {upside}",
        v.method
    )
}

/// user 提示词：本公司的事实块 + 随实时财报有无切换的数字纪律。`provider_ok=false` 时**硬性禁止**
/// 任何具体财务数字（对齐 composer 里 `hasLiveFin` 为假的分支——研究质量病灶的封堵点）。
#[must_use]
pub fn build_user_prompt(ctx: &AnswerContext) -> String {
    let name = ctx.name_zh.unwrap_or(&ctx.panel.ticker);

    let mut out = String::new();
    out.push_str(&format!("研究对象：{name}（{}）\n", ctx.panel.ticker));

    if !ctx.history.is_empty() {
        out.push_str(
            "\n== 本次会话此前几轮问答（仅供理解代词/实体指代，不得引用其中任何数字作为本轮核对依据——本轮所有数字以下方事实块为准）==\n",
        );
        let recent = ctx.history.iter().rev().take(3).collect::<Vec<_>>();
        for turn in recent.into_iter().rev() {
            out.push_str(&format!(
                "用户问：{}\n上轮作答摘要：{}\n\n",
                turn.question,
                truncate_chars(&turn.answer, 300)
            ));
        }
    }

    out.push_str(&format!("用户问题：{}\n", ctx.question));
    out.push_str(depth_directive(ctx.depth));
    out.push('\n');
    out.push_str(&facts_block(ctx));
    out
}

/// 聊天/报告主链使用的增强提示词：在本轮事实块之后追加显式降级约束与跨会话定性记忆。
/// 旧记忆不进入 `FactsRegistry`，数字已在 [`memory_prompt_block`] 中遮蔽。
#[must_use]
pub fn build_research_user_prompt(
    ctx: &AnswerContext,
    memory: Option<&CompanyMemory>,
    peer_anchor: Option<&PeerAnchor>,
    degradation: ResearchDegradation,
) -> String {
    let mut out = build_user_prompt(ctx);
    out.push_str(&supplemental_research_context(
        memory,
        peer_anchor,
        degradation,
    ));
    out
}

pub(crate) fn supplemental_research_context(
    memory: Option<&CompanyMemory>,
    peer_anchor: Option<&PeerAnchor>,
    degradation: ResearchDegradation,
) -> String {
    let mut out = String::new();
    if let Some(peer) = peer_anchor {
        out.push_str(&peer_anchor_block("已核到的同业倍数对照", peer));
    }
    match degradation {
        ResearchDegradation::PeerComparisonOnly => out.push_str(
            "\n降级规则：本轮公司自身估值未能成立，只回答同业倍数分布与可比性限制；\
             不给目标价、估值区间或隐含上涨空间。\n",
        ),
        ResearchDegradation::QualitativeOnly => out.push_str(
            "\n降级规则：本轮一手财务仍未核到，只做定性研究并列出待验证项；\
             不给任何财务数字或估值结论。\n",
        ),
        ResearchDegradation::None => {}
    }
    out.push_str(&memory_prompt_block(memory));
    out
}

pub(crate) fn compare_recovery_context(
    label: &str,
    peer_anchor: Option<&PeerAnchor>,
    degradation: ResearchDegradation,
) -> String {
    let mut out = String::new();
    if let Some(peer) = peer_anchor {
        out.push_str(&peer_anchor_block(
            &format!("{label} 已核到的同业倍数对照"),
            peer,
        ));
    }
    match degradation {
        ResearchDegradation::PeerComparisonOnly => out.push_str(&format!(
            "\n{label} 降级规则：自身估值未能成立，只比较同业倍数分布与可比性限制，\
             不给该公司的目标价、估值区间或隐含上涨空间。\n"
        )),
        ResearchDegradation::QualitativeOnly => out.push_str(&format!(
            "\n{label} 降级规则：一手财务仍未核到，只做定性比较，不给该公司的财务数字或估值结论。\n"
        )),
        ResearchDegradation::None => {}
    }
    out
}

fn peer_anchor_block(heading: &str, peer: &PeerAnchor) -> String {
    let multiple = match peer.multiple_type {
        MultipleType::Pe => "PE",
        MultipleType::EvSales => "EV/Sales",
    };
    format!(
        "\n\n== {heading} ==\n\
         {multiple} 分位：p25 {}x / 中位 {}x / p75 {}x；样本公司（{}）。\n\
         这些数字只描述同业分布；公司自身估值基数缺失时，不得据此反推目标价。\n",
        peer.p25,
        peer.median,
        peer.p75,
        peer.tickers.join("、")
    )
}

/// 单公司「已核到的事实」块——`build_user_prompt` 与深度报告提示词（`report.rs`）共用同一份
/// 事实格式化，确保报告引用的数字与聊天回答核对同一份 `FactsRegistry`，不会各拼一套口径。
pub(crate) fn facts_block(ctx: &AnswerContext) -> String {
    let has_live_fin = ctx.financials.provider_ok;
    let mut out = String::new();

    out.push_str("== 已核到的事实（只用这些数字）==\n");
    out.push_str(&format!("现价：{}\n", field(ctx.market.price, "")));
    out.push_str(&valuation_line(ctx.panel));
    out.push('\n');
    out.push_str(&format!(
        "连通数据源：{}\n",
        if ctx.panel.connected_sources.is_empty() {
            "无（本轮多为定性判断）".to_string()
        } else {
            ctx.panel.connected_sources.join("、")
        }
    ));

    if has_live_fin {
        out.push_str("\n== 已核到的实时财报（财务数字只能引用这里）==\n");
        let cur = ctx.financials.currency.as_deref().unwrap_or("");
        out.push_str(&format!("营收：{}\n", field(ctx.financials.revenue, cur)));
        out.push_str(&format!(
            "净利润：{}\n",
            field(ctx.financials.net_income, cur)
        ));
        out.push_str(&format!("EPS：{}\n", field(ctx.financials.eps, "")));
        out.push_str(&format!(
            "净利率：{}\n",
            field(ctx.financials.net_margin, "%")
        ));
        out.push_str(&format!(
            "毛利率：{}\n",
            field(ctx.financials.gross_margin, "%")
        ));
        out.push_str(&format!(
            "自由现金流：{}\n",
            field(ctx.financials.free_cash_flow, cur)
        ));
        out.push_str(
            "\n本轮有实时财报：必须用上面的真实数字支撑财务判断，不要再写「未核到完整三表」。",
        );
    } else {
        out.push_str(
            "\n本轮无实时财报：严禁给出任何具体财务数字或其估算值（收入/利润/EPS/利润率/现金流的\n\
             绝对值，以及「约」「大约」「行业常见范围」这类措辞都不行），只能定性描述赚钱机制与\n\
             风险并说明置信度下降；要数字就明说「需核最新财报」。",
        );
    }

    if let Some(hv) = &ctx.financials.historical_valuation {
        out.push_str("\n\n== 已核到的历史估值分位（近5年月度PE，仅美股）==\n");
        out.push_str(&format!("当前分位：{}\n", field(hv.percentile, "%")));
        out.push_str(&format!("当前倍数：{}\n", field(hv.latest, "x")));
        // p25/中位/p75 是估值「历史 PE 分位」法的锚点，必须逐个给出具体倍数：估值假设行里
        // 会提到这三个分位，事实块若只给 min/max/median，模型想引用 p25/p75 时无数可依，
        // 就会自己凑一个近似值（实测 GOOGL 中位 28.12x 被写成 28.17x，直接触发护栏硬失败）。
        out.push_str(&format!(
            "四分位：p25 {} / 中位 {} / p75 {}\n",
            field(hv.p25, "x"),
            field(hv.median, "x"),
            field(hv.p75, "x")
        ));
        out.push_str(&format!(
            "历史区间：{} ~ {}\n",
            field(hv.min, "x"),
            field(hv.max, "x")
        ));
        out.push_str("引用上述任一倍数时必须原样照抄，不得四舍五入或换算。\n");
    }

    if !ctx.filings.is_empty() {
        out.push_str("\n\n== 已核到的最新公司公告（SEC filings，可引用表单类型与日期）==\n");
        for filing in ctx.filings {
            let date = filing.filed_date.as_deref().unwrap_or("未核到日期");
            out.push_str(&format!(
                "{} · {date} · {}\n",
                filing.form, filing.source_url
            ));
        }
    }

    if !ctx.evidence.is_empty() {
        out.push_str(&evidence_block(ctx.evidence));
    }
    out
}

/// 网页证据段——每条编号，标题/来源/日期/片段/URL。首行硬性纪律：证据是二手信息，只做定性
/// 支撑，做定性论断时须标注来源编号或 URL；证据里的数字**不得**当作已核财务数字（与一手财报
/// 冲突时以财报为准，无一手财报时仍适用「本轮无实时财报」的数字禁令）。
pub(crate) fn evidence_block(evidence: &[Evidence]) -> String {
    let mut out = String::from(
        "\n\n== 已核到的网页证据（二手来源，仅供定性支撑）==\n\
         纪律：做定性论断时须标注来源编号或 URL；以下片段里的数字属二手信息，不得当作已核\n\
         财务数字引用（财务数字仍只认上方一手财报块，无一手财报则仍禁给具体财务数字）。\n",
    );
    for (i, e) in evidence.iter().enumerate() {
        let title = if e.title.is_empty() {
            "(无标题)"
        } else {
            &e.title
        };
        let domain = e.source_domain.as_deref().unwrap_or("来源域名未核到");
        let date = e.published_date.as_deref().unwrap_or("日期未核到");
        out.push_str(&format!("[{}] {title}（{domain} · {date}）\n", i + 1));
        if !e.snippet.is_empty() {
            out.push_str(&format!("{}\n", e.snippet));
        }
        out.push_str(&format!("来源：{}\n", e.url));
    }
    out
}

/// 对比研究一腿的事实——与 [`AnswerContext`] 同源，但刻意不含 `history`：两家公司各自
/// 独立取数，任何一方都不该被历史问答里提到的第三方数字污染。
pub struct CompareLegContext<'a> {
    pub name_zh: Option<&'a str>,
    pub panel: &'a DecisionPanel,
    pub market: &'a MarketSnapshot,
    pub financials: &'a Financials,
    pub evidence: &'a [Evidence],
}

/// 单腿事实块——与 `build_user_prompt` 里的"已核到的事实"段落同构，供对比 prompt 各标一份。
fn leg_facts_block(label: &str, ctx: &CompareLegContext) -> String {
    let name = ctx.name_zh.unwrap_or(&ctx.panel.ticker);
    let mut out = format!("=== {label}：{name}（{}）===\n", ctx.panel.ticker);
    out.push_str(&format!("现价：{}\n", field(ctx.market.price, "")));
    out.push_str(&valuation_line(ctx.panel));
    out.push('\n');
    if ctx.financials.provider_ok {
        let cur = ctx.financials.currency.as_deref().unwrap_or("");
        out.push_str(&format!("营收：{}\n", field(ctx.financials.revenue, cur)));
        out.push_str(&format!(
            "净利润：{}\n",
            field(ctx.financials.net_income, cur)
        ));
        out.push_str(&format!(
            "净利率：{}\n",
            field(ctx.financials.net_margin, "%")
        ));
    } else {
        out.push_str("本轮无实时财报：这一方严禁给出任何具体财务数字或估算值，只能定性判断。\n");
    }
    if !ctx.evidence.is_empty() {
        let prefix = if label == "公司A" { 'A' } else { 'B' };
        out.push_str(&format!(
            "\n--- {label} 独立网页证据（只支撑本公司定性判断，引用格式 [{prefix}1]）---\n"
        ));
        for (index, evidence) in ctx.evidence.iter().enumerate() {
            out.push_str(&format!(
                "[{prefix}{}] {} | {}\n{}\n来源：{}\n",
                index + 1,
                evidence.title,
                evidence.published_date.as_deref().unwrap_or("日期未核到"),
                evidence.snippet,
                evidence.url
            ));
        }
    }
    out
}

/// 对比研究 user 提示词：两腿事实各标各的名字/代码，且首行硬性禁止互相借用数字。
#[must_use]
pub fn build_compare_user_prompt(
    question: &str,
    primary: &CompareLegContext,
    peer: &CompareLegContext,
) -> String {
    let mut out = String::new();
    out.push_str(&format!("用户问题（对比研究）：{question}\n\n"));
    out.push_str(
        "铁律：以下两家公司的事实完全独立核实，绝不允许把一家的数字当成另一家的数字引用、\n\
         也不允许凭常识/记忆给出未在对应事实块里出现的数字。你在作答里提到任何具体数字时，\n\
         必须明确说明这个数字属于哪家公司（用公司名或代码标注），不得含糊带过。\n\
         网页证据同样不可串腿：公司A只引用 [A1]…，公司B只引用 [B1]…；每个定性结论都在句末\n\
         标对应来源号，绝不把 A 来源拿来证明 B，反之亦然。\n\n",
    );
    out.push_str(&leg_facts_block("公司A", primary));
    out.push('\n');
    out.push_str(&leg_facts_block("公司B", peer));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DecisionPanel, ResolvedCompany};
    use echo_domain::{Company, MarketSnapshot, display_valuation};
    use rust_decimal_macros::dec;

    fn panel_with(price: Option<Decimal>, provider_ok: bool) -> (DecisionPanel, Financials) {
        let company = ResolvedCompany {
            ticker: "AAPL".into(),
            name_zh: Some("苹果".into()),
            company: Company {
                price,
                ..Default::default()
            },
        };
        let market = MarketSnapshot {
            price,
            ..Default::default()
        };
        let financials = Financials {
            provider_ok,
            revenue: provider_ok.then(|| dec!(383000)),
            net_income: provider_ok.then(|| dec!(97000)),
            eps: provider_ok.then(|| dec!(6.13)),
            currency: Some("USD".into()),
            ..Default::default()
        };
        let valuation = display_valuation(&company.company, &market, &financials, None);
        let panel = crate::build_panel(&company, &market, &financials, None, &[]);
        let _ = valuation;
        (panel, financials)
    }

    #[test]
    fn system_prompt_states_no_trade_advice() {
        let sys = build_system_prompt();
        assert!(sys.contains("不构成投资建议"));
        assert!(sys.contains("买入/卖出"));
    }

    #[test]
    fn no_live_financials_forbids_concrete_numbers() {
        let (panel, financials) = panel_with(Some(dec!(190)), false);
        let market = MarketSnapshot {
            price: Some(dec!(190)),
            ..Default::default()
        };
        let prompt = build_user_prompt(&AnswerContext {
            question: "苹果现在贵不贵？",
            name_zh: Some("苹果"),
            panel: &panel,
            market: &market,
            financials: &financials,
            filings: &[],
            evidence: &[],
            depth: ResearchDepth::Standard,
            history: &[],
        });
        assert!(prompt.contains("严禁给出任何具体财务数字"));
        assert!(!prompt.contains("已核到的实时财报"));
        assert!(prompt.contains("研究对象：苹果（AAPL）"));
    }

    #[test]
    fn live_financials_block_appears_and_lifts_ban() {
        let (panel, financials) = panel_with(Some(dec!(190)), true);
        let market = MarketSnapshot {
            price: Some(dec!(190)),
            ..Default::default()
        };
        let prompt = build_user_prompt(&AnswerContext {
            question: "苹果利润质量如何？",
            name_zh: Some("苹果"),
            panel: &panel,
            market: &market,
            financials: &financials,
            filings: &[],
            evidence: &[],
            depth: ResearchDepth::Standard,
            history: &[],
        });
        assert!(prompt.contains("已核到的实时财报"));
        assert!(prompt.contains("383000USD"));
        assert!(prompt.contains("必须用上面的真实数字"));
        assert!(!prompt.contains("严禁给出任何具体财务数字"));
    }

    #[test]
    fn missing_price_renders_unverified_not_zero() {
        let (panel, financials) = panel_with(None, false);
        let market = MarketSnapshot::default();
        let prompt = build_user_prompt(&AnswerContext {
            question: "现价多少？",
            name_zh: None,
            panel: &panel,
            market: &market,
            financials: &financials,
            filings: &[],
            evidence: &[],
            depth: ResearchDepth::Standard,
            history: &[],
        });
        assert!(prompt.contains("现价：未核到"));
    }

    #[test]
    fn filings_block_cites_form_and_source_url() {
        let (panel, financials) = panel_with(Some(dec!(190)), false);
        let market = MarketSnapshot {
            price: Some(dec!(190)),
            ..Default::default()
        };
        let filings = vec![Filing {
            form: "10-K".into(),
            filed_date: Some("2026-05-01".into()),
            source_url: "https://www.sec.gov/example".into(),
        }];
        let prompt = build_user_prompt(&AnswerContext {
            question: "最近有什么公告？",
            name_zh: Some("苹果"),
            panel: &panel,
            market: &market,
            financials: &financials,
            filings: &filings,
            evidence: &[],
            depth: ResearchDepth::Standard,
            history: &[],
        });
        assert!(prompt.contains("已核到的最新公司公告"));
        assert!(prompt.contains("10-K"));
        assert!(prompt.contains("2026-05-01"));
        assert!(prompt.contains("https://www.sec.gov/example"));
    }

    #[test]
    fn evidence_block_renders_sources_and_keeps_number_ban_when_no_live_fin() {
        let (panel, financials) = panel_with(Some(dec!(190)), false);
        let market = MarketSnapshot {
            price: Some(dec!(190)),
            ..Default::default()
        };
        let evidence = vec![Evidence {
            title: "Apple services moat widens".into(),
            url: "https://reuters.com/tech/apple".into(),
            snippet: "服务收入占比继续提升，生态锁定增强。".into(),
            published_date: Some("2026-07-01".into()),
            source_domain: Some("reuters.com".into()),
        }];
        let prompt = build_user_prompt(&AnswerContext {
            question: "护城河还稳吗？",
            name_zh: Some("苹果"),
            panel: &panel,
            market: &market,
            financials: &financials,
            filings: &[],
            evidence: &evidence,
            depth: ResearchDepth::Standard,
            history: &[],
        });
        assert!(prompt.contains("已核到的网页证据"));
        assert!(prompt.contains("[1] Apple services moat widens（reuters.com · 2026-07-01）"));
        assert!(prompt.contains("来源：https://reuters.com/tech/apple"));
        assert!(prompt.contains("须标注来源编号或 URL"));
        // 证据在场也绝不解除「无实时财报→禁具体财务数字」的封堵。
        assert!(prompt.contains("严禁给出任何具体财务数字"));
    }

    #[test]
    fn depth_directive_changes_answer_style() {
        let (panel, financials) = panel_with(Some(dec!(190)), true);
        let market = MarketSnapshot {
            price: Some(dec!(190)),
            ..Default::default()
        };
        let ctx = |depth| AnswerContext {
            question: "护城河如何？",
            name_zh: Some("苹果"),
            panel: &panel,
            market: &market,
            financials: &financials,
            filings: &[],
            evidence: &[],
            depth,
            history: &[],
        };
        assert!(build_user_prompt(&ctx(ResearchDepth::Deep)).contains("系统展开"));
        assert!(build_user_prompt(&ctx(ResearchDepth::Brief)).contains("直接精简"));
        let standard = build_user_prompt(&ctx(ResearchDepth::Standard));
        assert!(!standard.contains("系统展开") && !standard.contains("直接精简"));
    }

    #[test]
    fn empty_evidence_omits_the_block() {
        let (panel, financials) = panel_with(Some(dec!(190)), false);
        let market = MarketSnapshot {
            price: Some(dec!(190)),
            ..Default::default()
        };
        let prompt = build_user_prompt(&AnswerContext {
            question: "护城河？",
            name_zh: Some("苹果"),
            panel: &panel,
            market: &market,
            financials: &financials,
            filings: &[],
            evidence: &[],
            depth: ResearchDepth::Standard,
            history: &[],
        });
        assert!(!prompt.contains("已核到的网页证据"));
    }

    #[test]
    fn empty_filings_omit_the_block() {
        let (panel, financials) = panel_with(Some(dec!(190)), false);
        let market = MarketSnapshot {
            price: Some(dec!(190)),
            ..Default::default()
        };
        let prompt = build_user_prompt(&AnswerContext {
            question: "最近有什么公告？",
            name_zh: Some("苹果"),
            panel: &panel,
            market: &market,
            financials: &financials,
            filings: &[],
            evidence: &[],
            depth: ResearchDepth::Standard,
            history: &[],
        });
        assert!(!prompt.contains("已核到的最新公司公告"));
    }

    #[test]
    fn history_carries_pronouns_but_is_labeled_not_a_fact_source() {
        let (panel, financials) = panel_with(Some(dec!(190)), false);
        let market = MarketSnapshot {
            price: Some(dec!(190)),
            ..Default::default()
        };
        let history = vec![PriorTurn {
            question: "苹果的护城河是什么？".into(),
            answer: "苹果的护城河主要在生态锁定与服务收入占比提升。".into(),
        }];
        let prompt = build_user_prompt(&AnswerContext {
            question: "它的估值贵不贵？",
            name_zh: Some("苹果"),
            panel: &panel,
            market: &market,
            financials: &financials,
            filings: &[],
            evidence: &[],
            depth: ResearchDepth::Standard,
            history: &history,
        });
        assert!(prompt.contains("不得引用其中任何数字作为本轮核对依据"));
        assert!(prompt.contains("苹果的护城河是什么？"));
        assert!(prompt.contains("生态锁定"));
    }

    #[test]
    fn history_answer_beyond_limit_is_truncated() {
        let (panel, financials) = panel_with(Some(dec!(190)), false);
        let market = MarketSnapshot {
            price: Some(dec!(190)),
            ..Default::default()
        };
        let long_answer = "壁".repeat(400);
        let history = vec![PriorTurn {
            question: "护城河？".into(),
            answer: long_answer.clone(),
        }];
        let prompt = build_user_prompt(&AnswerContext {
            question: "它的估值贵不贵？",
            name_zh: Some("苹果"),
            panel: &panel,
            market: &market,
            financials: &financials,
            filings: &[],
            evidence: &[],
            depth: ResearchDepth::Standard,
            history: &history,
        });
        assert!(!prompt.contains(&long_answer));
        assert!(prompt.contains('…'));
    }

    #[test]
    fn compare_prompt_labels_each_leg_and_forbids_cross_borrowing() {
        let (apple_panel, apple_financials) = panel_with(Some(dec!(190)), true);
        let apple_market = MarketSnapshot {
            price: Some(dec!(190)),
            ..Default::default()
        };
        let (tencent_panel, tencent_financials) = {
            let company = ResolvedCompany {
                ticker: "0700.HK".into(),
                name_zh: Some("腾讯".into()),
                company: Company {
                    price: Some(dec!(300)),
                    ..Default::default()
                },
            };
            let market = MarketSnapshot {
                price: Some(dec!(300)),
                ..Default::default()
            };
            let financials = Financials {
                provider_ok: true,
                revenue: Some(dec!(160000)),
                net_income: Some(dec!(30000)),
                currency: Some("HKD".into()),
                ..Default::default()
            };
            let panel = crate::build_panel(&company, &market, &financials, None, &[]);
            (panel, financials)
        };
        let tencent_market = MarketSnapshot {
            price: Some(dec!(300)),
            ..Default::default()
        };

        let prompt = build_compare_user_prompt(
            "苹果和腾讯谁的利润质量更好？",
            &CompareLegContext {
                name_zh: Some("苹果"),
                panel: &apple_panel,
                market: &apple_market,
                financials: &apple_financials,
                evidence: &[Evidence {
                    title: "Apple source".into(),
                    url: "https://example.com/apple".into(),
                    snippet: "Apple evidence".into(),
                    ..Default::default()
                }],
            },
            &CompareLegContext {
                name_zh: Some("腾讯"),
                panel: &tencent_panel,
                market: &tencent_market,
                financials: &tencent_financials,
                evidence: &[Evidence {
                    title: "Tencent source".into(),
                    url: "https://example.com/tencent".into(),
                    snippet: "Tencent evidence".into(),
                    ..Default::default()
                }],
            },
        );
        assert!(prompt.contains("绝不允许把一家的数字当成另一家的数字"));
        assert!(prompt.contains("苹果（AAPL）"));
        assert!(prompt.contains("腾讯（0700.HK）"));
        assert!(prompt.contains("383000USD"));
        assert!(prompt.contains("160000HKD"));
        assert!(prompt.contains("[A1] Apple source"));
        assert!(prompt.contains("[B1] Tencent source"));
        assert!(prompt.contains("绝不把 A 来源拿来证明 B"));
    }
}
