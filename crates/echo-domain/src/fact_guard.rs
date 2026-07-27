//! 数字级防幻觉护栏（R3）。
//!
//! 两个入口：
//!   * [`build_facts_registry`]：把已接地的结构化数据（行情/财报/估值/回购/内部人/历史分位…）
//!     按维度（货币金额/百分比/倍数/日期）收成一张"事实登记表"。
//!   * [`verify_answer_numbers`]：从模型正文抽数字逐个去登记表核对，给 pass / soft（未核到，
//!     不拦截）/ hard（符号相反、数量级差过大、日期查无、币种张冠李戴）。
//!
//! 设计原则：**宁可漏报不可误报**（默认 soft，只有符号相反/数量级≥阈值/显式日期查无/
//! 换算后仍对不上的币种标注才升级 hard）。
//!
//! 与 JS 的实质差异：**零二进制浮点**。选"量级上最接近的事实"原文用 `log10` 距离，这里改用
//! Decimal 比值 `max(v/f, f/v)` 取最小——同序、且不引入任何浮点（红线 4）。

use crate::valuation::{Financials, MarketSnapshot, Valuation};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use fancy_regex::Regex;

// 展示级近似汇率，只用于跨币种数量级核对，不参与任何金融计算。
fn fx_to_hkd(currency: &str) -> Option<Decimal> {
    match currency.to_ascii_uppercase().as_str() {
        "HKD" => Some(dec!(1)),
        "CNY" => Some(dec!(1.08)),
        "USD" => Some(dec!(7.8)),
        _ => None,
    }
}

fn cn_unit(unit: &str) -> Option<Decimal> {
    match unit {
        "万亿" => Some(dec!(1000000000000)),
        "亿" => Some(dec!(100000000)),
        "万" => Some(dec!(10000)),
        _ => None,
    }
}

fn resolve_currency(token: &str) -> Option<String> {
    let t = token.trim();
    let mapped = match t {
        "港元" | "港币" | "HK$" | "HKD" => "HKD",
        "美元" | "US$" | "$" | "USD" => "USD",
        "人民币" | "RMB" | "¥" | "CNY" => "CNY",
        _ => match t.to_ascii_uppercase().as_str() {
            "HKD" => "HKD",
            "USD" => "USD",
            "CNY" | "RMB" => "CNY",
            _ => return None,
        },
    };
    Some(mapped.to_string())
}

const AMOUNT_KEYWORDS: &[&str] = &[
    "现价",
    "收盘价",
    "目标价",
    "看空",
    "中性",
    "看多",
    // 估值区间是现在的主要输出，模型多写成"基准估值区间 234.51"而非登记表里的"估值中性"。
    // 不收这两个词，裸数字会被整个跳过——连 soft 都不记，等于估值数字完全不受核对。
    "基准",
    "估值",
    "市值",
    "收入",
    "营收",
    "净利",
    "毛利",
    "经营利润",
    "现金流",
    "EPS",
    "每股",
    "回购",
    "分红",
    "净现金",
    "净债务",
    "成本",
    "止损",
    "止盈",
];

/// compactNumber() 输出（"3.92 万亿" / "2099.21 亿"）反解析成原始数值。
#[must_use]
pub fn parse_compact_amount(s: &str) -> Option<Decimal> {
    static RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^(-?\d+(?:\.\d+)?)\s*(万亿|亿|万)?$").unwrap());
    let s = s.trim();
    let caps = RE.captures(s).ok().flatten()?;
    let n: Decimal = caps.get(1)?.as_str().parse().ok()?;
    match caps.get(2).map(|m| m.as_str()) {
        Some(unit) => cn_unit(unit).map(|u| n * u),
        None => Some(n),
    }
}

/// 展示级近似汇率换算（跟 CNY→HKD=1.08、HKD/USD≈1/7.8 同一套常量）。
#[must_use]
pub fn convert_currency(value: Decimal, from: &str, to: &str) -> Option<Decimal> {
    let f = fx_to_hkd(from)?;
    let t = fx_to_hkd(to)?;
    Some((value * f) / t)
}

// ───────────────────────── registry ─────────────────────────

/// 一条按维度分桶的事实。
#[derive(Clone, Debug)]
pub struct Fact {
    pub value: Decimal,
    pub label: String,
    pub source: String,
}

/// 一条可核对日期事实。
#[derive(Clone, Debug)]
pub struct DateFact {
    pub iso: String,
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub quarter: u32,
    pub label: String,
    pub source: String,
}

/// 按维度分桶的事实登记表。
#[derive(Clone, Debug, Default)]
pub struct FactsRegistry {
    pub ticker: String,
    pub native_currency: String,
    pub amounts: BTreeMap<String, Vec<Fact>>,
    pub percents: Vec<Fact>,
    pub multiples: Vec<Fact>,
    pub dates: Vec<DateFact>,
}

impl FactsRegistry {
    fn push_amount(
        &mut self,
        value: Option<Decimal>,
        currency: Option<&str>,
        label: &str,
        source: &str,
    ) {
        let Some(v) = value else { return };
        let cur = currency
            .map(str::to_ascii_uppercase)
            .unwrap_or_else(|| self.native_currency.clone());
        self.amounts.entry(cur).or_default().push(Fact {
            value: v,
            label: label.into(),
            source: source.into(),
        });
    }
    fn push_percent(&mut self, value: Option<Decimal>, label: &str, source: &str) {
        if let Some(v) = value {
            self.percents.push(Fact {
                value: v,
                label: label.into(),
                source: source.into(),
            });
        }
    }
    fn push_multiple(&mut self, value: Option<Decimal>, label: &str, source: &str) {
        if let Some(v) = value {
            self.multiples.push(Fact {
                value: v,
                label: label.into(),
                source: source.into(),
            });
        }
    }
    fn push_date(&mut self, iso_like: Option<&str>, label: &str, source: &str) {
        let Some(s) = iso_like else { return };
        static RE: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"^(\d{4})-(\d{2})-(\d{2})").unwrap());
        if let Ok(Some(caps)) = RE.captures(s) {
            let year: i32 = caps.get(1).unwrap().as_str().parse().unwrap();
            let month: u32 = caps.get(2).unwrap().as_str().parse().unwrap();
            let day: u32 = caps.get(3).unwrap().as_str().parse().unwrap();
            self.dates.push(DateFact {
                iso: format!("{year:04}-{month:02}-{day:02}"),
                year,
                month,
                day,
                quarter: month.div_ceil(3),
                label: label.into(),
                source: source.into(),
            });
        }
    }
}

/// `pct_string` 的逆运算——把 `"-27.8%"` 解回 `-27.8`。估值把空间存成展示字符串，
/// 登记表要的是数值。解析不出就返回 `None`（不登记好过登错）。
fn parse_pct(text: Option<&str>) -> Option<Decimal> {
    text?.trim().trim_end_matches('%').trim().parse().ok()
}

/// 用户持仓（真实 DB 记录，不用自由文本）。
#[derive(Clone, Debug, Default)]
pub struct Position {
    pub avg_cost: Option<Decimal>,
    pub stop_loss: Option<Decimal>,
    pub take_profit: Option<Decimal>,
}

/// 登记表构建输入——每个源都可缺省（缺省即不登记，绝不占位）。
#[derive(Default)]
pub struct RegistrySources<'a> {
    pub ticker: &'a str,
    pub native_currency: Option<&'a str>,
    pub market: Option<&'a MarketSnapshot>,
    pub financials: Option<&'a Financials>,
    pub valuation: Option<&'a Valuation>,
    pub earnings_next_date: Option<&'a str>,
    pub position: Option<&'a Position>,
}

/// 把已接地的结构化数据收成事实登记表。构建顺序体现事实源优先级（结构化 > 估值 > 日历），
/// 但匹配时不分先后，只看"这个数字是否落在对应维度任一事实的容差内"。
#[must_use]
pub fn build_facts_registry(sources: &RegistrySources) -> FactsRegistry {
    let native_currency = sources
        .native_currency
        .map(str::to_string)
        .or_else(|| sources.market.and_then(|m| m.currency.clone()))
        .or_else(|| sources.financials.and_then(|f| f.currency.clone()))
        .unwrap_or_else(|| "HKD".into())
        .to_ascii_uppercase();

    let mut reg = FactsRegistry {
        ticker: sources.ticker.into(),
        native_currency: native_currency.clone(),
        ..Default::default()
    };

    // 结构化事实：优先级最高。
    if let Some(m) = sources.market.filter(|m| m.is_ok()) {
        let cur = m.currency.as_deref();
        reg.push_amount(m.price, cur, "现价", "marketSnapshot");
        reg.push_percent(m.change_percent, "今日涨跌幅", "marketSnapshot");
        reg.push_multiple(m.pe, "PE", "marketSnapshot");
        reg.push_percent(m.dividend_yield, "股息率", "marketSnapshot");
        reg.push_amount(m.market_cap, cur, "市值", "marketSnapshot");
    }

    if let Some(f) = sources.financials.filter(|f| f.provider_ok) {
        let cur_owned = f
            .currency
            .clone()
            .unwrap_or_else(|| native_currency.clone());
        let cur = Some(cur_owned.as_str());
        reg.push_amount(f.revenue, cur, "收入", "financialsData");
        reg.push_amount(f.gross_profit, cur, "毛利", "financialsData");
        reg.push_amount(f.operating_income, cur, "经营利润", "financialsData");
        reg.push_amount(f.net_income, cur, "净利润", "financialsData");
        reg.push_amount(f.free_cash_flow, cur, "自由现金流", "financialsData");
        reg.push_amount(f.operating_cash_flow, cur, "经营现金流", "financialsData");
        reg.push_amount(
            f.cash_and_equivalents,
            cur,
            "现金及等价物",
            "financialsData",
        );
        // 任何喂给模型的数字都必须先登记，否则模型如实引用反被判 hard（F-4a 教训）。
        reg.push_amount(f.net_cash, cur, "净现金", "financialsData");
        reg.push_amount(f.net_debt, cur, "净债务", "financialsData");
        reg.push_amount(f.dividend_paid, cur, "分红", "financialsData");
        reg.push_amount(f.repurchase_of_stock, cur, "回购金额", "financialsData");
        reg.push_amount(f.eps, cur, "EPS", "financialsData");
        reg.push_percent(f.revenue_growth, "收入增速", "financialsData");
        reg.push_percent(f.gross_margin, "毛利率", "financialsData");
        reg.push_percent(f.operating_margin, "经营利润率", "financialsData");
        reg.push_percent(f.net_margin, "净利率", "financialsData");
        reg.push_percent(f.profit_growth, "利润增速", "financialsData");
        reg.push_percent(f.return_on_equity, "ROE", "financialsData");
        reg.push_percent(f.return_on_assets, "ROA", "financialsData");
        reg.push_multiple(f.pe, "PE", "financialsData");
        reg.push_multiple(f.forward_pe, "Forward PE", "financialsData");
        reg.push_multiple(f.pb, "PB", "financialsData");
        reg.push_date(f.period.as_deref(), "财报期", "financialsData.period");

        // F-4a 内部人净买卖：只登金额/日期，不登股数（股数量级与货币金额不同源）。
        if let Some(ins) = &f.insider_activity {
            reg.push_amount(
                ins.net_value,
                cur,
                "内部人净买卖金额",
                "financialsData.insiderActivity",
            );
            reg.push_date(
                ins.last_transaction_at.as_deref(),
                "内部人最近交易日",
                "financialsData.insiderActivity",
            );
        }
        // F-4b 港股回购：登总代价与最近购回日，不登股数。
        if !f.hk_buybacks.is_empty() {
            let total: Decimal = f
                .hk_buybacks
                .iter()
                .filter_map(|r| r.total_consideration)
                .sum();
            let bb_cur = f.hk_buybacks[0].currency.as_deref().or(cur);
            reg.push_amount(
                Some(total),
                bb_cur,
                "港股回购总代价",
                "financialsData.hkBuybacks",
            );
            reg.push_date(
                f.hk_buybacks[0].trade_date.as_deref(),
                "最近购回交易日",
                "financialsData.hkBuybacks",
            );
        }
    }

    // F-5 历史估值分位——**刻意在三表 `provider_ok` 门控之外**。它来自历史行情 + 年报 EPS，
    // 与当季三表能否取到毫无关系：三表 429/缺源时这段分位照样有值，事实块也照样把它喂给
    // 模型（那一段同样不受 provider_ok 约束）。此前登记被门控挡住，于是模型如实照抄分位却
    // 全被判失败——正是本文件上方那条"喂给模型的数字必须先登记"的 F-4a 教训再犯一次。
    // p25/p75/latest 是估值「历史 PE 分位」法的锚点，与 min/max/median 一并登记。
    if let Some(hv) = sources
        .financials
        .and_then(|f| f.historical_valuation.as_ref())
    {
        reg.push_percent(
            hv.percentile,
            "历史估值分位",
            "financialsData.historicalValuation",
        );
        reg.push_multiple(
            hv.min,
            "历史PE区间低值",
            "financialsData.historicalValuation",
        );
        reg.push_multiple(
            hv.max,
            "历史PE区间高值",
            "financialsData.historicalValuation",
        );
        reg.push_multiple(
            hv.median,
            "历史PE中位",
            "financialsData.historicalValuation",
        );
        reg.push_multiple(hv.p25, "历史PE p25", "financialsData.historicalValuation");
        reg.push_multiple(hv.p75, "历史PE p75", "financialsData.historicalValuation");
        reg.push_multiple(
            hv.latest,
            "历史PE当前倍数",
            "financialsData.historicalValuation",
        );
    }

    // 估值输出：次优先级——自己的确定性计算，可信但排在原始事实之后。
    if let Some(v) = sources
        .valuation
        .filter(|v| v.cannot_value_reason.is_none())
    {
        let nc = Some(native_currency.as_str());
        reg.push_amount(v.bear, nc, "估值看空", "valuation");
        reg.push_amount(v.base, nc, "估值中性", "valuation");
        reg.push_amount(v.bull, nc, "估值看多", "valuation");
        reg.push_amount(v.current_price, nc, "现价", "valuation");
        // 相对现价空间同样出现在事实块的估值行里，必须登记。它通常为负（估值低于现价 =
        // 这股票贵），而百分比桶里其余多是正值，漏登时模型如实引用会被判"符号相反"的硬失败。
        reg.push_percent(parse_pct(v.upside.as_deref()), "估值空间", "valuation");
        reg.push_percent(
            parse_pct(v.downside.as_deref()),
            "估值下行空间",
            "valuation",
        );
        // 赔率刻意不进 multiples 桶（与 PE/EV-Sales 完全不同尺度，会造成误报）。
        for m in &v.method_detail {
            reg.push_amount(
                Some(m.bear),
                nc,
                &format!("{} 看空", m.name),
                "valuation.methodDetail",
            );
            reg.push_amount(
                Some(m.base),
                nc,
                &format!("{} 中性", m.name),
                "valuation.methodDetail",
            );
            reg.push_amount(
                Some(m.bull),
                nc,
                &format!("{} 看多", m.name),
                "valuation.methodDetail",
            );
        }
    }

    // 财报日历。
    if let Some(next) = sources.earnings_next_date {
        reg.push_date(Some(next), "下一业绩日", "earnings");
    }

    // 持仓盈亏：真实持仓 + 现价现算。
    if let (Some(pos), Some(m)) = (sources.position, sources.market.filter(|m| m.is_ok())) {
        if let (Some(price), Some(cost)) = (m.price, pos.avg_cost) {
            if !cost.is_zero() {
                reg.push_percent(
                    Some(((price - cost) / cost) * dec!(100)),
                    "持仓浮动盈亏",
                    "position",
                );
            }
        }
        let nc = Some(native_currency.as_str());
        reg.push_amount(pos.avg_cost, nc, "持仓成本", "position");
        reg.push_amount(pos.stop_loss, nc, "止损线", "position");
        reg.push_amount(pos.take_profit, nc, "止盈线", "position");
    }

    reg
}

/// 把另一份登记表并入主表（对比任务用）——按维度追加，不覆盖主体事实。
pub fn merge_facts_registry(into: &mut FactsRegistry, from: FactsRegistry) {
    for (currency, facts) in from.amounts {
        if !facts.is_empty() {
            into.amounts.entry(currency).or_default().extend(facts);
        }
    }
    into.percents.extend(from.percents);
    into.multiples.extend(from.multiples);
    into.dates.extend(from.dates);
}

// ───────────────────────── extraction ─────────────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
enum DatePrecision {
    Day,
    Quarter,
}

#[derive(Clone, Debug)]
enum Candidate {
    Percent {
        value: Decimal,
        raw: String,
        index: usize,
    },
    Multiple {
        value: Decimal,
        raw: String,
        index: usize,
    },
    Amount {
        value: Decimal,
        currency: Option<String>,
        raw: String,
        index: usize,
    },
    Date {
        year: i32,
        month: u32,
        day: u32,
        quarter: u32,
        precision: DatePrecision,
        raw: String,
        index: usize,
    },
}

impl Candidate {
    fn index(&self) -> usize {
        match self {
            Candidate::Percent { index, .. }
            | Candidate::Multiple { index, .. }
            | Candidate::Amount { index, .. }
            | Candidate::Date { index, .. } => *index,
        }
    }
}

/// 取 byte 位置 `idx` 之前最后 `n` 个字符（按 char 安全切片，避免切断多字节汉字）。
fn chars_before(text: &str, idx: usize, n: usize) -> String {
    let head = &text[..idx];
    let mut chars: Vec<char> = head.chars().collect();
    let start = chars.len().saturating_sub(n);
    chars.drain(..start);
    chars.into_iter().collect()
}

/// 取 byte 位置 `idx` 之后前 `n` 个字符。
fn chars_after(text: &str, idx: usize, n: usize) -> String {
    text[idx..].chars().take(n).collect()
}

const DIRECTION_WORDS: &[&str] = &[
    "亏损扩大",
    "收窄",
    "走低",
    "回落",
    "放缓",
    "转负",
    "缩水",
    "萎缩",
    "降",
    "跌",
    "滑",
    "减",
];

fn ends_with_direction(before: &str) -> bool {
    DIRECTION_WORDS.iter().any(|w| before.ends_with(w))
}

/// 从正文抽候选数字。分优先级跑正则、用字符区间去重（防 "15.75x" 里的 "15.75" 被裸数字规则再抓）。
fn extract_numbers(text: &str) -> Vec<Candidate> {
    static RE_PERCENT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?<![\d.])([-+]?\d+(?:\.\d+)?)\s*%").unwrap());
    static RE_MULTIPLE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(\d+(?:\.\d+)?)\s*[xX倍]").unwrap());
    static RE_ISO: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(\d{4})-(\d{2})-(\d{2})").unwrap());
    static RE_CN_DATE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(\d{4})年(\d{1,2})月(\d{1,2})日").unwrap());
    static RE_QUARTER: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(\d{4})\s*年?\s*(?:Q([1-4])|第([一二三四])季度)").unwrap());
    static RE_AMOUNT_UNIT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?<![\d.])(-?\d+(?:\.\d+)?)\s*(万亿|亿|万)(港元|美元|人民币|HKD|USD|CNY|元)?")
            .unwrap()
    });
    static RE_BARE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?<![\d.])(-?\d+(?:\.\d+)?)").unwrap());

    let mut consumed: Vec<(usize, usize)> = Vec::new();
    let is_free = |consumed: &[(usize, usize)], s: usize, e: usize| {
        !consumed.iter().any(|&(cs, ce)| s < ce && e > cs)
    };
    let mut out: Vec<Candidate> = Vec::new();

    // 百分比（含中文降幅语义补号）。
    for cap in RE_PERCENT.captures_iter(text).flatten() {
        let whole = cap.get(0).unwrap();
        let (s, e) = (whole.start(), whole.end());
        let g1 = cap.get(1).unwrap().as_str();
        let mut value: Decimal = g1.parse().unwrap_or(Decimal::ZERO);
        if !g1.starts_with(['-', '+']) {
            let before = chars_before(text, whole.start(), 6);
            if ends_with_direction(&before) {
                value = -value.abs();
            }
        }
        out.push(Candidate::Percent {
            value,
            raw: whole.as_str().into(),
            index: s,
        });
        consumed.push((s, e));
    }
    // 倍数。
    for cap in RE_MULTIPLE.captures_iter(text).flatten() {
        let whole = cap.get(0).unwrap();
        let (s, e) = (whole.start(), whole.end());
        if !is_free(&consumed, s, e) {
            continue;
        }
        let value: Decimal = cap
            .get(1)
            .unwrap()
            .as_str()
            .parse()
            .unwrap_or(Decimal::ZERO);
        out.push(Candidate::Multiple {
            value,
            raw: whole.as_str().into(),
            index: s,
        });
        consumed.push((s, e));
    }
    // ISO 日期。
    for cap in RE_ISO.captures_iter(text).flatten() {
        let whole = cap.get(0).unwrap();
        let (s, e) = (whole.start(), whole.end());
        let year = cap.get(1).unwrap().as_str().parse().unwrap();
        let month = cap.get(2).unwrap().as_str().parse().unwrap();
        let day = cap.get(3).unwrap().as_str().parse().unwrap();
        out.push(Candidate::Date {
            year,
            month,
            day,
            quarter: 0,
            precision: DatePrecision::Day,
            raw: whole.as_str().into(),
            index: s,
        });
        consumed.push((s, e));
    }
    // 中文日期。
    for cap in RE_CN_DATE.captures_iter(text).flatten() {
        let whole = cap.get(0).unwrap();
        let (s, e) = (whole.start(), whole.end());
        if !is_free(&consumed, s, e) {
            continue;
        }
        let year = cap.get(1).unwrap().as_str().parse().unwrap();
        let month = cap.get(2).unwrap().as_str().parse().unwrap();
        let day = cap.get(3).unwrap().as_str().parse().unwrap();
        out.push(Candidate::Date {
            year,
            month,
            day,
            quarter: 0,
            precision: DatePrecision::Day,
            raw: whole.as_str().into(),
            index: s,
        });
        consumed.push((s, e));
    }
    // 季度。
    for cap in RE_QUARTER.captures_iter(text).flatten() {
        let whole = cap.get(0).unwrap();
        let (s, e) = (whole.start(), whole.end());
        if !is_free(&consumed, s, e) {
            continue;
        }
        let year = cap.get(1).unwrap().as_str().parse().unwrap();
        let quarter = if let Some(q) = cap.get(2) {
            q.as_str().parse().unwrap()
        } else {
            let cn = cap.get(3).unwrap().as_str();
            ("一二三四"
                .chars()
                .position(|c| cn.starts_with(c))
                .unwrap_or(0)
                + 1) as u32
        };
        out.push(Candidate::Date {
            year,
            month: 0,
            day: 0,
            quarter,
            precision: DatePrecision::Quarter,
            raw: whole.as_str().into(),
            index: s,
        });
        consumed.push((s, e));
    }
    // 带单位金额。
    for cap in RE_AMOUNT_UNIT.captures_iter(text).flatten() {
        let whole = cap.get(0).unwrap();
        let (s, e) = (whole.start(), whole.end());
        if !is_free(&consumed, s, e) {
            continue;
        }
        // "3451万股"是股数不是金额——紧跟"股/份"就跳过。
        let after = chars_after(text, e, 2);
        if after.starts_with('股') || after.starts_with('份') {
            consumed.push((s, e));
            continue;
        }
        let n: Decimal = cap
            .get(1)
            .unwrap()
            .as_str()
            .parse()
            .unwrap_or(Decimal::ZERO);
        let unit = cap.get(2).unwrap().as_str();
        let currency = cap.get(3).and_then(|m| resolve_currency(m.as_str()));
        out.push(Candidate::Amount {
            value: n * cn_unit(unit).unwrap_or(Decimal::ONE),
            currency,
            raw: whole.as_str().into(),
            index: s,
        });
        consumed.push((s, e));
    }
    // 裸数字：必须有相邻货币标签或财务关键词窗口命中才算候选。
    static RE_TAG: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(港元|美元|人民币|HKD|USD|CNY|HK\$|US\$|\$|¥)").unwrap());
    for cap in RE_BARE.captures_iter(text).flatten() {
        let whole = cap.get(0).unwrap();
        let (s, e) = (whole.start(), whole.end());
        if !is_free(&consumed, s, e) {
            continue;
        }
        let before = chars_before(text, s, 10);
        let after = chars_after(text, e, 6);
        let window = format!("{before}{after}");
        let tag = RE_TAG
            .captures(&window)
            .ok()
            .flatten()
            .and_then(|c| c.get(1).and_then(|m| resolve_currency(m.as_str())));
        let has_tag = RE_TAG.is_match(&window).unwrap_or(false);
        let has_keyword = AMOUNT_KEYWORDS.iter().any(|k| before.contains(k));
        if !has_tag && !has_keyword {
            continue;
        }
        let value: Decimal = cap
            .get(1)
            .unwrap()
            .as_str()
            .parse()
            .unwrap_or(Decimal::ZERO);
        out.push(Candidate::Amount {
            value,
            currency: tag,
            raw: whole.as_str().into(),
            index: s,
        });
        consumed.push((s, e));
    }

    out.sort_by_key(Candidate::index);
    out
}

// ───────────────────────── matching ─────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Soft,
    Hard,
}

struct MatchResult {
    verdict: Verdict,
    fact: Option<(String, Decimal)>,
    reason: Option<String>,
}

struct Tol {
    rel: Decimal,
    abs: Decimal,
    magnitude: Decimal,
}

const SIGN_FLIP_RATIO: Decimal = dec!(1.25);

fn abs(d: Decimal) -> Decimal {
    d.abs()
}

/// 在同维度桶里找容差内的事实。选"量级最接近"改用 Decimal 比值 `max(v/f, f/v)` 取最小
/// （等价于 |log10| 距离排序，但零浮点）。
fn match_in_bucket(value: Decimal, bucket: &[Fact], tol: &Tol) -> MatchResult {
    if bucket.is_empty() {
        return MatchResult {
            verdict: Verdict::Soft,
            fact: None,
            reason: None,
        };
    }
    for fact in bucket {
        let diff = abs(value - fact.value);
        let t = tol.abs.max(abs(fact.value) * tol.rel);
        if diff <= t {
            return MatchResult {
                verdict: Verdict::Pass,
                fact: Some((fact.label.clone(), fact.value)),
                reason: None,
            };
        }
    }
    // 候选值为 0 一律 soft（"零负债"是合法陈述，且比值无意义）。
    if value.is_zero() {
        return MatchResult {
            verdict: Verdict::Soft,
            fact: None,
            reason: None,
        };
    }
    // 选量级最接近的非零事实：min over facts of max(|v|/|f|, |f|/|v|)。
    let mut best: Option<&Fact> = None;
    let mut best_ratio = Decimal::MAX;
    for fact in bucket {
        if fact.value.is_zero() {
            continue;
        }
        let a = abs(value);
        let b = abs(fact.value);
        let ratio = if a >= b { a / b } else { b / a };
        if ratio < best_ratio {
            best_ratio = ratio;
            best = Some(fact);
        }
    }
    let Some(best) = best else {
        return MatchResult {
            verdict: Verdict::Soft,
            fact: None,
            reason: None,
        };
    };
    let mag_ratio = abs(value) / abs(best.value); // best.value 非零
    let same_magnitude =
        mag_ratio >= (Decimal::ONE / SIGN_FLIP_RATIO) && mag_ratio <= SIGN_FLIP_RATIO;
    let signs_differ = !value.is_zero()
        && !best.value.is_zero()
        && value.is_sign_negative() != best.value.is_sign_negative();
    if same_magnitude && signs_differ {
        return MatchResult {
            verdict: Verdict::Hard,
            fact: Some((best.label.clone(), best.value)),
            reason: Some(format!(
                "符号相反（最接近的事实是\"{}\"={}）",
                best.label,
                best.value.normalize()
            )),
        };
    }
    if mag_ratio >= tol.magnitude || mag_ratio <= (Decimal::ONE / tol.magnitude) {
        let times = if mag_ratio >= tol.magnitude {
            mag_ratio
        } else {
            Decimal::ONE / mag_ratio
        };
        return MatchResult {
            verdict: Verdict::Hard,
            fact: Some((best.label.clone(), best.value)),
            reason: Some(format!(
                "数量级相差 {} 倍以上（最接近的事实是\"{}\"={}）",
                times.round_dp(0).normalize(),
                best.label,
                best.value.normalize()
            )),
        };
    }
    MatchResult {
        verdict: Verdict::Soft,
        fact: Some((best.label.clone(), best.value)),
        reason: None,
    }
}

fn percent_tol() -> Tol {
    Tol {
        rel: dec!(0),
        abs: dec!(0.3),
        magnitude: dec!(10),
    }
}
fn multiple_tol() -> Tol {
    Tol {
        rel: dec!(0.05),
        abs: dec!(0.3),
        magnitude: dec!(30),
    }
}
fn amount_tol() -> Tol {
    Tol {
        rel: dec!(0.02),
        abs: dec!(0),
        magnitude: dec!(1000),
    }
}
fn amount_tol_cross() -> Tol {
    Tol {
        rel: dec!(0.08),
        abs: dec!(0),
        magnitude: dec!(1000),
    }
}

fn match_amount(value: Decimal, stated: Option<&str>, reg: &FactsRegistry) -> MatchResult {
    let currency = stated
        .map(str::to_string)
        .unwrap_or_else(|| reg.native_currency.clone());
    let is_native = currency == reg.native_currency;
    let empty = Vec::new();
    let direct_bucket = reg.amounts.get(&currency).unwrap_or(&empty);
    let direct = match_in_bucket(value, direct_bucket, &amount_tol());
    if direct.verdict == Verdict::Pass {
        return direct;
    }
    // 非本币桶的 hard 不采信（常只有零星异量级事实，易误判）。
    if direct.verdict == Verdict::Hard && is_native {
        return direct;
    }
    if let Some(stated) = stated.filter(|s| *s != reg.native_currency) {
        let native_bucket = reg.amounts.get(&reg.native_currency).unwrap_or(&empty);
        if let Some(converted) = convert_currency(value, stated, &reg.native_currency) {
            let cross = match_in_bucket(converted, native_bucket, &amount_tol_cross());
            if cross.verdict == Verdict::Pass {
                return cross;
            }
            // 换算后仍对不上：看原始数值是否精确撞本币事实——像"数字对、币种标签错"。
            let mislabel = match_in_bucket(value, native_bucket, &amount_tol());
            if mislabel.verdict == Verdict::Pass {
                let (label, fv) = mislabel.fact.clone().unwrap();
                return MatchResult {
                    verdict: Verdict::Hard,
                    fact: mislabel.fact,
                    reason: Some(format!(
                        "疑似币种标注错误：数值与 {} 口径的\"{}\"（{}）吻合，但标注成了 {}",
                        reg.native_currency,
                        label,
                        fv.normalize(),
                        stated
                    )),
                };
            }
            if cross.verdict == Verdict::Hard {
                return cross;
            }
        }
        return MatchResult {
            verdict: Verdict::Soft,
            fact: direct.fact,
            reason: None,
        };
    }
    direct
}

/// (year,month,day) → 距 1970-01-01 的天数（Howard Hinnant days_from_civil），用于日期容差。
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y } as i64;
    let m = m as i64;
    let d = d as i64;
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn match_date(cand: &Candidate, reg: &FactsRegistry) -> MatchResult {
    let Candidate::Date {
        year,
        month,
        day,
        quarter,
        precision,
        ..
    } = cand
    else {
        unreachable!()
    };
    if *precision == DatePrecision::Day {
        let cand_days = days_from_civil(*year, *month, *day);
        if reg
            .dates
            .iter()
            .any(|f| (days_from_civil(f.year, f.month, f.day) - cand_days).abs() <= 1)
        {
            return MatchResult {
                verdict: Verdict::Pass,
                fact: None,
                reason: None,
            };
        }
        let q = month.div_ceil(3);
        if reg.dates.iter().any(|f| f.year == *year && f.quarter == q) {
            return MatchResult {
                verdict: Verdict::Soft,
                fact: None,
                reason: Some("季度对得上但具体日期不一致".into()),
            };
        }
        return MatchResult {
            verdict: Verdict::Hard,
            fact: None,
            reason: Some("给出的具体日期在已核数据里找不到对应记录".into()),
        };
    }
    let hit = reg
        .dates
        .iter()
        .any(|f| f.year == *year && (*quarter == 0 || f.quarter == *quarter));
    if hit {
        MatchResult {
            verdict: Verdict::Pass,
            fact: None,
            reason: None,
        }
    } else {
        MatchResult {
            verdict: Verdict::Soft,
            fact: None,
            reason: None,
        }
    }
}

// ───────────────────────── verify ─────────────────────────

/// 一条被核对的数字。
#[derive(Clone, Debug)]
pub struct CheckedNumber {
    pub raw: String,
    pub dimension: &'static str,
    pub verdict: Verdict,
    pub matched_fact: Option<(String, Decimal)>,
    pub reason: Option<String>,
}

/// 核对结果。
#[derive(Clone, Debug, Default)]
pub struct VerifyReport {
    pub checked: Vec<CheckedNumber>,
    pub soft_count: usize,
    pub hard_count: usize,
}

impl VerifyReport {
    #[must_use]
    pub fn has_hard_fail(&self) -> bool {
        self.hard_count > 0
    }
    #[must_use]
    pub fn pass_count(&self) -> usize {
        self.checked
            .iter()
            .filter(|c| c.verdict == Verdict::Pass)
            .count()
    }
}

/// 校验正文里的数字。只检查"来源："之前的正文，并跳过紧跟"北京时间"的时间戳。
#[must_use]
pub fn verify_answer_numbers(text: &str, reg: &FactsRegistry) -> VerifyReport {
    static RE_SOURCE_SPLIT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\n\n?来源[:：]").unwrap());
    let scan_text = RE_SOURCE_SPLIT
        .split(text)
        .next()
        .map(|r| r.unwrap_or(text))
        .unwrap_or(text);

    let mut checked = Vec::new();
    for cand in extract_numbers(scan_text) {
        // 跳过"北京时间 …"后的生成时间戳（不是待核实的财务日期）。
        if let Candidate::Date { index, .. } = &cand {
            let before = chars_before(scan_text, *index, 6);
            if before.trim_end().ends_with("北京时间") {
                continue;
            }
        }
        let (dimension, result) = match &cand {
            Candidate::Percent { value, .. } => (
                "percent",
                match_in_bucket(*value, &reg.percents, &percent_tol()),
            ),
            Candidate::Multiple { value, .. } => (
                "multiple",
                match_in_bucket(*value, &reg.multiples, &multiple_tol()),
            ),
            Candidate::Amount {
                value, currency, ..
            } => ("amount", match_amount(*value, currency.as_deref(), reg)),
            Candidate::Date { .. } => ("date", match_date(&cand, reg)),
        };
        let raw = match &cand {
            Candidate::Percent { raw, .. }
            | Candidate::Multiple { raw, .. }
            | Candidate::Amount { raw, .. }
            | Candidate::Date { raw, .. } => raw.clone(),
        };
        checked.push(CheckedNumber {
            raw,
            dimension,
            verdict: result.verdict,
            matched_fact: result.fact,
            reason: result.reason,
        });
    }

    let soft_count = checked
        .iter()
        .filter(|c| c.verdict == Verdict::Soft)
        .count();
    let hard_count = checked
        .iter()
        .filter(|c| c.verdict == Verdict::Hard)
        .count();
    VerifyReport {
        checked,
        soft_count,
        hard_count,
    }
}

// ─────────────────────── 定性引用护栏 ───────────────────────

/// 定性论断的引用完整性核对结果。**这是引用完整性，不是语义核对**——只核"标了几号来源、
/// 有没有引用不存在的来源号、有证据却零引用"，绝不声称验证了论断与来源内容一致（那需要
/// 语义比对，成本与误报都高，不在护栏范围）。与数字护栏互补：数字护栏保证数字，引用护栏
/// 让定性判断至少可追溯到某条真实来源、且不虚构来源号。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CitationReport {
    /// 本轮提供给模型的证据条数。
    pub evidence_count: usize,
    /// 答案里出现且在范围内（1..=evidence_count）的来源号，去重升序。
    pub cited: Vec<usize>,
    /// 引用了超出证据条数的来源号（虚构引用）——真实的完整性失败，升 hard。
    pub out_of_range: Vec<usize>,
    /// 有证据（evidence_count>0）但答案一个来源号都没标——定性论断裸奔，soft 提示。
    pub ungrounded: bool,
}

impl CitationReport {
    #[must_use]
    pub fn cited_count(&self) -> usize {
        self.cited.len()
    }
    /// 引用了不存在的来源号——护栏级硬信号（模型虚构了来源）。
    #[must_use]
    pub fn has_hallucinated_citation(&self) -> bool {
        !self.out_of_range.is_empty()
    }
}

/// 抽取答案里形如 `[3]` 或 `来源[3]` 的引用号，按是否落在 `1..=evidence_count` 分成已引用与
/// 越界两组。`evidence_count == 0`（本轮没给证据）时不产生任何 hard/soft——无证据不苛求引用。
#[must_use]
pub fn verify_answer_citations(text: &str, evidence_count: usize) -> CitationReport {
    // 本轮无证据（数字驱动意图/供应商降级）：不承诺任何来源，故任何 `[N]` 都不算虚构、也不判
    // 裸奔。直接返回空报告，避免把此时的编号误判成越界引用。
    if evidence_count == 0 {
        return CitationReport::default();
    }
    static RE_CITE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?:来源\s*)?\[(\d{1,2})\]").unwrap());
    let mut cited = BTreeSet::new();
    let mut out_of_range = BTreeSet::new();
    for caps in RE_CITE.captures_iter(text).flatten() {
        let Some(n) = caps.get(1).and_then(|m| m.as_str().parse::<usize>().ok()) else {
            continue;
        };
        if n == 0 {
            continue;
        }
        if n <= evidence_count {
            cited.insert(n);
        } else {
            out_of_range.insert(n);
        }
    }
    // 无证据时（evidence_count==0）越界组恒空：任何 [N] 都算不上"虚构来源号"，因为本轮压根
    // 没承诺任何来源；此时也不判 ungrounded。
    let ungrounded = evidence_count > 0 && cited.is_empty();
    CitationReport {
        evidence_count,
        cited: cited.into_iter().collect(),
        out_of_range: out_of_range.into_iter().collect(),
        ungrounded,
    }
}

/// SOFT_FLAG 提示文案（低调、不拦截）——soft/full 模式追加在正文末尾。
#[must_use]
pub fn build_soft_note(report: &VerifyReport) -> String {
    if report.soft_count == 0 && report.hard_count == 0 {
        return String::new();
    }
    let mut parts = Vec::new();
    if report.soft_count > 0 {
        parts.push(format!(
            "{} 处数字未能与已核实数据直接核对",
            report.soft_count
        ));
    }
    if report.hard_count > 0 {
        parts.push(format!(
            "{} 处存在明显不一致（符号/数量级/日期）",
            report.hard_count
        ));
    }
    format!(
        "\n\n> 提示：{}。判断依据请以事实块和来源链接为准。",
        parts.join("，")
    )
}

/// 定向重答用：把 hard-fail 数字整理成可读问题清单（纯格式化）。
#[must_use]
pub fn render_hard_fail_issues(report: &VerifyReport) -> String {
    report
        .checked
        .iter()
        .filter(|c| c.verdict == Verdict::Hard)
        .map(|c| {
            let tail = c
                .matched_fact
                .as_ref()
                .map(|(l, v)| format!("（可能应为\"{}\"={}）", l, v.normalize()))
                .unwrap_or_default();
            format!(
                "- \"{}\"：{}{}",
                c.raw,
                c.reason
                    .clone()
                    .unwrap_or_else(|| "与已核实数据不一致".into()),
                tail
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::valuation::{Financials, HistoricalValuation};

    /// 历史 PE 分位来自历史行情 + 年报 EPS，与当季三表是两条独立管线。三表 429/缺源时
    /// （`provider_ok=false`）事实块照样把分位喂给模型，登记表就必须照样收录——否则模型
    /// 如实照抄反被判失败。这是估值区间上线后暴露的真实故障（AAPL 一度 0 过 / 4 核）。
    #[test]
    fn historical_percentiles_registered_even_without_live_statements() {
        let fin = Financials {
            provider_ok: false,
            historical_valuation: Some(HistoricalValuation {
                percentile: Some(dec!(98.28)),
                min: Some(dec!(21.31)),
                max: Some(dec!(44.53)),
                median: Some(dec!(31.70)),
                p25: Some(dec!(28.02)),
                p75: Some(dec!(36.80)),
                latest: Some(dec!(43.93)),
            }),
            ..Default::default()
        };
        let reg = build_facts_registry(&RegistrySources {
            ticker: "AAPL",
            financials: Some(&fin),
            ..Default::default()
        });
        let answer = "当前 43.93x 的 PE 处于近 5 年 98.28% 分位，远超历史中位 31.70x，\
                      p25 为 28.02x、p75 为 36.80x。";
        let report = verify_answer_numbers(answer, &reg);
        assert_eq!(
            report.hard_count, 0,
            "如实照抄的分位不得判硬失败: {report:?}"
        );
        assert_eq!(report.soft_count, 0, "分位应全部核上: {report:?}");
        assert_eq!(report.pass_count(), 5);
    }

    /// 估值空间是负数（估值低于现价 = 贵），而百分比桶里其余多为正值。漏登时模型如实引用
    /// 会撞上"符号相反"判成硬失败——AAPL 实测就栽在这一处。
    #[test]
    fn valuation_upside_is_registered_so_negative_space_is_not_a_sign_flip() {
        let valuation = Valuation {
            method: "历史 PE 分位".into(),
            bear: Some(dec!(207.28)),
            base: Some(dec!(234.51)),
            bull: Some(dec!(272.01)),
            upside: Some("-27.8%".into()),
            downside: Some("-36.2%".into()),
            current_price: Some(dec!(324.98)),
            methods: vec!["历史 PE 分位".into()],
            method_detail: Vec::new(),
            key_assumptions: Vec::new(),
            sensitivity: Vec::new(),
            stage_aware: false,
            stage: None,
            data_suspect: false,
            cannot_value_reason: None,
        };
        let reg = build_facts_registry(&RegistrySources {
            ticker: "AAPL",
            native_currency: Some("USD"),
            valuation: Some(&valuation),
            ..Default::default()
        });
        let report = verify_answer_numbers("基准估值区间 234.51 对应现价空间为 -27.8%。", &reg);
        assert_eq!(report.hard_count, 0, "如实引用估值空间不得判硬: {report:?}");
        assert_eq!(report.pass_count(), 2);
    }

    #[test]
    fn parse_pct_round_trips_display_strings() {
        assert_eq!(parse_pct(Some("-27.8%")), Some(dec!(-27.8)));
        assert_eq!(parse_pct(Some("12%")), Some(dec!(12)));
        assert_eq!(parse_pct(None), None);
        assert_eq!(parse_pct(Some("未核到")), None);
    }

    fn tencent_registry() -> FactsRegistry {
        let fin = Financials {
            provider_ok: true,
            currency: Some("CNY".into()),
            revenue: Some(dec!(160_000_000_000)),
            net_income: Some(dec!(48_000_000_000)),
            net_margin: Some(dec!(30.4)),
            net_cash: Some(dec!(33_374_000_000)),
            ..Default::default()
        };
        let market = MarketSnapshot {
            price: Some(dec!(400)),
            currency: Some("HKD".into()),
            change_percent: Some(dec!(1.2)),
            ..Default::default()
        };
        build_facts_registry(&RegistrySources {
            ticker: "0700.HK",
            market: Some(&market),
            financials: Some(&fin),
            ..Default::default()
        })
    }

    #[test]
    fn faithful_net_margin_passes() {
        let reg = tencent_registry();
        let r = verify_answer_numbers("净利率 30.4%，本季稳健。", &reg);
        assert_eq!(r.hard_count, 0);
        assert_eq!(r.pass_count(), 1);
    }

    #[test]
    fn sign_flip_is_hard() {
        // 登记的净利率是 +30.4%，模型写 -30.4% 应判符号相反 hard。
        let reg = tencent_registry();
        let r = verify_answer_numbers("净利率 -30.4%。", &reg);
        assert_eq!(r.hard_count, 1);
    }

    #[test]
    fn chinese_decline_semantics_not_false_hard() {
        // "收入同比下滑10.9%"是中文降幅写法（无负号），不得因符号被误判。
        let fin = Financials {
            provider_ok: true,
            currency: Some("CNY".into()),
            revenue_growth: Some(dec!(-10.9)),
            ..Default::default()
        };
        let reg = build_facts_registry(&RegistrySources {
            ticker: "X",
            financials: Some(&fin),
            ..Default::default()
        });
        let r = verify_answer_numbers("收入同比下滑10.9%。", &reg);
        assert_eq!(r.hard_count, 0);
        assert_eq!(r.pass_count(), 1);
    }

    #[test]
    fn share_count_not_treated_as_amount() {
        // "回购3451.51万股"是股数不是金额，不得进金额核对。
        let reg = tencent_registry();
        let r = verify_answer_numbers("累计回购3451.51万股。", &reg);
        assert!(!r.checked.iter().any(|c| c.dimension == "amount"));
    }

    #[test]
    fn source_section_is_not_scanned() {
        let reg = tencent_registry();
        // 正文无数字，"来源"段里的日期不应被扫描。
        let r = verify_answer_numbers("腾讯基本面稳健。\n\n来源：yahoo（2026-07-17）", &reg);
        assert_eq!(r.checked.len(), 0);
    }

    #[test]
    fn citations_in_range_are_collected_deduped() {
        let r = verify_answer_citations("护城河见来源[1]，AI 风险见 [3] 与来源[3]。", 5);
        assert_eq!(r.cited, vec![1, 3]);
        assert_eq!(r.cited_count(), 2);
        assert!(r.out_of_range.is_empty());
        assert!(!r.ungrounded);
        assert!(!r.has_hallucinated_citation());
    }

    #[test]
    fn out_of_range_citation_is_hallucinated_hard_signal() {
        // 只给了 2 条证据，却引用了 [5] → 虚构来源号。
        let r = verify_answer_citations("如来源[1] 与 [5] 所述。", 2);
        assert_eq!(r.cited, vec![1]);
        assert_eq!(r.out_of_range, vec![5]);
        assert!(r.has_hallucinated_citation());
    }

    #[test]
    fn evidence_present_but_zero_citations_is_ungrounded() {
        let r = verify_answer_citations("苹果护城河很稳，没有标注任何来源。", 4);
        assert!(r.cited.is_empty());
        assert!(r.ungrounded);
        assert!(!r.has_hallucinated_citation());
    }

    #[test]
    fn no_evidence_this_turn_never_flags() {
        // 本轮无证据（数字驱动意图/供应商降级）：任何 [N] 都不算虚构，也不判裸奔。
        let r = verify_answer_citations("PE 约 28 倍 [1]。", 0);
        assert!(r.cited.is_empty());
        assert!(r.out_of_range.is_empty());
        assert!(!r.ungrounded);
    }
}
