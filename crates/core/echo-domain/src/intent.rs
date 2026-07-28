//! 研究意图分类——纯规则、双语输入与稳定优先级。
//!
//! **顺序即优先级**：规则之间必然重叠（"深度研究一下它的护城河和估值"同时命中三条），
//! 数组顺序本身就是产品决策。总原则：产出形态 > 具体主题。每条规则的中英模式写在同一个
//! 正则里（中英双语是一等公民），迁移时一个字符都不改，正是为了让"只加中文忘了英文"在
//! review 时一眼可见。9 条规则里 deepResearch 必须最先判、valuation 必须排在
//! financialQuality 之前等，都是 2026-07-17 回归实测抓到的次序，勿重排。

use fancy_regex::Regex;
use std::sync::LazyLock;

/// 研究意图（九类）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResearchIntent {
    CompanyStatus,
    BusinessModel,
    Competitors,
    Moat,
    FinancialQuality,
    Valuation,
    RiskEvent,
    Falsify,
    DeepResearch,
}

impl ResearchIntent {
    /// 与前端/契约稳定的字符串标识（对齐 ROUTER_SYSTEM_PROMPT 的取值）。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CompanyStatus => "company_status",
            Self::BusinessModel => "business_model",
            Self::Competitors => "competitors",
            Self::Moat => "moat",
            Self::FinancialQuality => "financial_quality",
            Self::Valuation => "valuation",
            Self::RiskEvent => "risk_event",
            Self::Falsify => "falsify",
            Self::DeepResearch => "deep_research",
        }
    }
}

/// 研究深度：单点短答 / 多维标准 / 完整报告。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResearchDepth {
    Brief,
    Standard,
    Deep,
}

impl ResearchDepth {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Brief => "brief",
            Self::Standard => "standard",
            Self::Deep => "deep",
        }
    }
}

/// 一条路由决策。
#[derive(Clone, Debug)]
pub struct ResearchRoute {
    pub intent: ResearchIntent,
    pub depth: ResearchDepth,
    /// 确定性路由的置信度（≥0.7 时应用层跳过模型二次路由）。这是路由置信度、非金融量，用 f64。
    pub confidence: f64,
    pub multi_part: bool,
    pub source: &'static str,
    pub answer_style: &'static str,
    pub plan: Vec<&'static str>,
}

/// 顺序即优先级——每条 `(intent, 正则)`。正则前缀 `(?i)` 对齐 JS 的 `/i` 大小写不敏感。
static RULES: LazyLock<Vec<(ResearchIntent, Regex)>> = LazyLock::new(|| {
    let rules: &[(ResearchIntent, &str)] = &[
        (
            ResearchIntent::DeepResearch,
            r"(?i)深度研究|完整报告|研究报告|全面分析|深度分析|详细报告|[出写来给]\s*[一份]{0,2}\s*.{0,6}报告|full\s+(research\s+)?report|deep\s+(dive|research)|comprehensive\s+(analysis|report)|detailed\s+report|write\s+(me\s+)?an?\s+.{0,12}report",
        ),
        (
            ResearchIntent::Falsify,
            r"(?i)证伪|证伪条件|什么情况会(证伪|推翻)|什么会(证伪|推翻|让.{0,4}(看错|错))|哪些.{0,6}(会推翻|证伪)|看错|逻辑(被)?推翻|bear\s*case|what\s+would\s+(prove|make)\s+.{0,20}wrong|falsif|downside\s+case|invalidate\s+the\s+thesis",
        ),
        (
            ResearchIntent::RiskEvent,
            r"(?i)版号|关税|制裁|反垄断|罚款|处罚|停牌|退市|做空|集体诉讼|数据泄露|召回|暴雷|爆雷|限售|减持|tariff|sanction|antitrust|delist|short\s+(seller|report)|class\s+action|data\s+breach|recall\b|probe\b",
        ),
        (
            ResearchIntent::Competitors,
            r"(?i)竞争对手|竞品|同行|同业|可比公司|可比对象|竞争格局|市场格局|行业格局|替代品|谁在抢|和谁竞争|主要竞争|竞争压力|competitors?\b|rivals?\b|competitive\s+landscape|market\s+share|who\s+(are|is)\s+.{0,14}(compet|rival)|peer\s+group",
        ),
        (
            ResearchIntent::BusinessModel,
            r"(?i)靠什么赚钱|怎么赚钱|如何赚钱|盈利模式|商业模式|收入来源|主要收入|利润来源|赚的是什么钱|谁付钱|变现方式|谁给.{0,8}付钱|收入(主要)?靠|利润(主要)?靠|收入(结构|构成|拆分|分部)|靠.{0,10}还是.{0,10}[?？]?$|business\s+model|(how|where)\s+does\s+.{0,16}\s+make\s+money|revenue\s+(stream|source|mix|model|breakdown|split)|monetiz|who\s+pays",
        ),
        (
            ResearchIntent::Moat,
            r"(?i)护城河|竞争优势|壁垒|不可替代|垄断|网络效应|优势在哪|优势是什么|溢价能.{0,4}持续|\bmoat\b|competitive\s+(advantage|edge)|barrier\s+to\s+entry|network\s+effect|pricing\s+power|defensib|durable\s+advantage",
        ),
        (
            ResearchIntent::Valuation,
            r"(?i)估值|贵不贵|便宜|PE|PB|PS|市盈率|市净率|市销率|目标价|赔率|性价比|多少倍|几倍算|高估|低估|历史分位|\bvaluation\b|expensive|\bcheap\b|overvalu|undervalu|price\s+target|fair\s+value|trading\s+at\s+\d|\bmultiples?\b",
        ),
        (
            ResearchIntent::FinancialQuality,
            r"(?i)赚钱吗|赚不赚钱|能不能赚钱|能赚钱|赚不赚|是否赚钱|有没有赚钱|盈不盈利|盈利吗|赚钱能力|利润|毛利|净利|现金流|自由现金流|财务质量|经营质量|收入|亏损|还在亏|还亏|亏钱|亏不亏|亏吗|亏多少|扭亏|巨亏|减亏|盈亏|盈利|应收|存货|资本开支|回本|占比多少|增速|GMV|口径|profitab|margins?\b|cash\s*flow|\bfcf\b|revenue|earnings\s+quality|financials\b|loss[- ]making|burn\s+rate|receivable|inventory|capex|balance\s+sheet",
        ),
        (
            ResearchIntent::RiskEvent,
            r"(?i)为什么跌|为什么涨|下跌|大跌|暴跌|上涨|大涨|风险|监管|处罚|事故|事件|怎么了|关税|版号|制裁|会不会影响|why\s+(did|is|has)\s+.{0,16}(drop|fall|plunge|surge|jump|rally|rise|down|up)\b|sell-?off|\brisks?\b|regulat|lawsuit|investigat|tariff|sanction|what\s+happened",
        ),
    ];
    rules
        .iter()
        .map(|(intent, pat)| (*intent, Regex::new(pat).expect("intent rule regex")))
        .collect()
});

static EXPLICIT_BRIEF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)一句话|简单(说|讲|回答|看看)?|简短|直接说|只说结论|quick\s+answer|briefly|in\s+one\s+sentence").unwrap()
});
static EXPLICIT_DEEP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)深度|完整|全面|详细|系统性|从头到尾|研究报告|deep|comprehensive|detailed|full\s+report").unwrap()
});
static MULTI_PART: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)分别|逐一|同时|以及|并且|然后|对比|比较|vs|还要|另外|一方面.{0,40}另一方面|第一.{0,40}第二").unwrap()
});

fn matched(re: &Regex, text: &str) -> bool {
    re.is_match(text).unwrap_or(false)
}

/// 依优先级返回首个命中的意图；无命中兜底 `CompanyStatus`。
#[must_use]
pub fn classify_research_intent(question: &str) -> ResearchIntent {
    RULES
        .iter()
        .find(|(_, re)| matched(re, question))
        .map_or(ResearchIntent::CompanyStatus, |(intent, _)| *intent)
}

/// 按规则顺序返回全部去重命中的意图（route 用其数量判断多维/置信度）。
fn unique_intent_matches(text: &str) -> Vec<ResearchIntent> {
    let mut out: Vec<ResearchIntent> = Vec::new();
    for (intent, re) in RULES.iter() {
        if matched(re, text) && !out.contains(intent) {
            out.push(*intent);
        }
    }
    out
}

/// 该意图是否值得拉网页证据。定性、时效敏感的意图（现状/护城河/竞争/风险/证伪/深研）
/// 才触发——估值与财务质量是数字驱动、已有一手财报/同业/分位专属数据面，不拉二手网页避免
/// 引入噪音与不必要延迟。与 [`plan_research_stages`] 里点亮 `evidence` 阶段的意图集合保持
/// 同源语义（此处多含 `Falsify`：熊市/证伪论证同样需要当下证据），改一处须同步另一处。
#[must_use]
pub fn intent_wants_web_evidence(intent: ResearchIntent) -> bool {
    matches!(
        intent,
        ResearchIntent::CompanyStatus
            | ResearchIntent::Moat
            | ResearchIntent::Competitors
            | ResearchIntent::RiskEvent
            | ResearchIntent::Falsify
            | ResearchIntent::DeepResearch
    )
}

/// 阶段计划——前端等待态指示器逐条点亮（stage 名是与前端的稳定契约）。
#[must_use]
pub fn plan_research_stages(intent: ResearchIntent, _depth: ResearchDepth) -> Vec<&'static str> {
    let mut stages = vec!["routing", "resolving", "market_financials"];
    if intent_wants_web_evidence(intent) {
        stages.push("evidence");
    }
    if matches!(
        intent,
        ResearchIntent::CompanyStatus
            | ResearchIntent::Valuation
            | ResearchIntent::Falsify
            | ResearchIntent::DeepResearch
    ) {
        stages.push("valuation");
    }
    stages.push("generating");
    stages.push("fact_check");
    stages
}

/// 确定性首轮路由。高置信度显然问题不必付第二次模型调用；歧义问题刻意给低置信度，
/// 让应用层决定是否请模型做结构化路由。depth 控制取数与作答长度，不只是文案风格。
#[must_use]
pub fn route_research_intent(question: &str) -> ResearchRoute {
    let text = question.trim();
    let compact_length = text.chars().filter(|c| !c.is_whitespace()).count();
    let matches = unique_intent_matches(text);
    let intent = matches
        .first()
        .copied()
        .unwrap_or(ResearchIntent::CompanyStatus);
    let explicitly_deep = intent == ResearchIntent::DeepResearch || matched(&EXPLICIT_DEEP, text);
    let explicitly_brief = matched(&EXPLICIT_BRIEF, text);
    let multi_part = matches.len() > 1 || matched(&MULTI_PART, text);
    let naturally_brief = compact_length > 0
        && compact_length <= 34
        && intent != ResearchIntent::CompanyStatus
        && !multi_part;
    let depth = if explicitly_deep {
        ResearchDepth::Deep
    } else if explicitly_brief || naturally_brief {
        ResearchDepth::Brief
    } else {
        ResearchDepth::Standard
    };
    let confidence = match matches.len() {
        0 => 0.46,
        1 => 0.94,
        _ => 0.78,
    };
    let answer_style = match depth {
        ResearchDepth::Brief => "direct",
        ResearchDepth::Deep => "report",
        ResearchDepth::Standard => "research",
    };
    ResearchRoute {
        intent,
        depth,
        confidence,
        multi_part,
        source: "rules",
        answer_style,
        plan: plan_research_stages(intent, depth),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_research_wins_over_subtopics() {
        // "深度研究一下它的护城河和估值" 同时命中三条，但产出形态优先 → deep_research。
        let r = route_research_intent("深度研究一下它的护城河和估值");
        assert_eq!(r.intent, ResearchIntent::DeepResearch);
        assert_eq!(r.depth, ResearchDepth::Deep);
    }

    #[test]
    fn valuation_beats_financial_quality() {
        // "腾讯的估值和利润哪个先修复"主语是估值，不能被"利润"抢走。
        assert_eq!(
            classify_research_intent("腾讯的估值和利润哪个先修复"),
            ResearchIntent::Valuation
        );
    }

    #[test]
    fn english_is_first_class() {
        assert_eq!(
            classify_research_intent("what is the competitive landscape"),
            ResearchIntent::Competitors
        );
        assert_eq!(
            classify_research_intent("how does it make money"),
            ResearchIntent::BusinessModel
        );
        assert_eq!(
            classify_research_intent("write me a full research report"),
            ResearchIntent::DeepResearch
        );
    }

    #[test]
    fn short_focused_question_is_brief() {
        let r = route_research_intent("护城河在哪");
        assert_eq!(r.intent, ResearchIntent::Moat);
        assert_eq!(r.depth, ResearchDepth::Brief);
        assert!(r.confidence >= 0.9);
        assert!(
            r.plan.contains(&"evidence"),
            "brief 只缩短作答，不应让计划与真实证据 IO 脱节"
        );
    }

    #[test]
    fn bare_company_question_falls_back_to_status_standard() {
        let r = route_research_intent("苹果怎么样");
        assert_eq!(r.intent, ResearchIntent::CompanyStatus);
        // companyStatus 排除在 naturallyBrief 之外 → standard。
        assert_eq!(r.depth, ResearchDepth::Standard);
    }
}
