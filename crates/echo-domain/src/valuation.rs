//! 结构化多方法估值。
//!
//! 这里保留了原文件里全部硬啃出来的领域不变量，一条不丢：
//!   * **禁止"以现价为中心的 PE 带"**：`bear/base/bull = 现价 ×0.78/×1.0/×1.28` 会让
//!     base 恒等于现价、赔率恒为 ~1.3:1，是"没有估值"冒充"有估值"的自循环根因。
//!     缺可信口径就诚实返回 `cannot_value_reason`，绝不回退到零信息带子。
//!   * **亏损股走 EV/Sales 情景**：负 EPS × 负 PE 会"负负得正"拼出假带子，因此
//!     `pe > 0 && eps > 0` 是 PE 法的硬前置。
//!   * **EPS 年化护栏**：港股/A 股中报 EPS 是报告期累计值（非 TTM），
//!     `eps_annualized == Some(false)` 时禁止用它反推 PE。
//!   * **可信度护栏**：EV/Sales 带与市场隐含倍数量级脱节时判脏，宁可不估。
//!
//! 与 JS 版唯一的实质差异：金额/比率用 `rust_decimal::Decimal` 定点运算，
//! 二进制浮点被彻底逐出（红线 4）。

use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::Serialize;

/// 可比同业倍数类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum MultipleType {
    Pe,
    EvSales,
}

/// 同业锚点（compPeers 的 anchor）——真实市场对同阶段可比公司的定价分位。
#[derive(Clone, Debug)]
pub struct PeerAnchor {
    pub multiple_type: MultipleType,
    pub p25: Decimal,
    pub median: Decimal,
    pub p75: Decimal,
    pub n: usize,
    pub tickers: Vec<String>,
}

impl PeerAnchor {
    fn all_positive(&self) -> bool {
        self.p25 > Decimal::ZERO && self.median > Decimal::ZERO && self.p75 > Decimal::ZERO
    }
}

/// 唯一财务事实源（research.ts 的 `toDomainSources` 产出，估值/护栏/提示词共用同一份）。
#[derive(Clone, Debug, Default)]
pub struct Financials {
    pub provider_ok: bool,
    pub eps: Option<Decimal>,
    /// `Some(false)` 表示报告期累计 EPS（不得反推 PE）；`None` 视为已年化（对齐 JS 的 `!== false`）。
    pub eps_annualized: Option<bool>,
    pub forward_pe: Option<Decimal>,
    pub net_margin: Option<Decimal>,
    pub operating_margin: Option<Decimal>,
    pub revenue: Option<Decimal>,
    pub revenue_growth: Option<Decimal>,
    pub gross_margin: Option<Decimal>,
    pub shares_outstanding: Option<Decimal>,
    pub cash_and_equivalents: Option<Decimal>,
    pub total_debt: Option<Decimal>,
    pub net_cash: Option<Decimal>,
    pub free_cash_flow: Option<Decimal>,
    // ── 以下字段主要供数字护栏（fact_guard）登记事实用；估值只读上面那些。
    /// 记账币种（港股 HKD 报价 / CNY 记账时两者不同——护栏据此判断币种是否张冠李戴）。
    pub currency: Option<String>,
    pub gross_profit: Option<Decimal>,
    pub operating_income: Option<Decimal>,
    pub net_income: Option<Decimal>,
    pub operating_cash_flow: Option<Decimal>,
    pub net_debt: Option<Decimal>,
    pub dividend_paid: Option<Decimal>,
    pub repurchase_of_stock: Option<Decimal>,
    pub pe: Option<Decimal>,
    pub pb: Option<Decimal>,
    pub return_on_equity: Option<Decimal>,
    pub return_on_assets: Option<Decimal>,
    pub profit_growth: Option<Decimal>,
    /// 财报期（ISO，护栏登记为可核对日期）。
    pub period: Option<String>,
    /// 内部人净买卖（F-4a：登记金额与交易日，不登股数）。
    pub insider_activity: Option<InsiderActivity>,
    /// 港股回购（F-4b：登记总代价与最近购回日，不登股数）。
    pub hk_buybacks: Vec<HkBuyback>,
    /// 历史估值分位（F-5：分位/区间必须先登记再允许被引用）。
    pub historical_valuation: Option<HistoricalValuation>,
}

/// 下一次财报日历（Finnhub）——独立于三表，缺数即 `provider_ok = false`，绝不用陈旧值冒充。
#[derive(Clone, Debug, Default)]
pub struct EarningsCalendar {
    pub provider_ok: bool,
    pub next_date: Option<String>,
    pub quarter: Option<i32>,
    pub year: Option<i32>,
    pub eps_estimate: Option<Decimal>,
    pub revenue_estimate: Option<Decimal>,
}

/// 一份公司公告/披露（Finnhub SEC filings index）——只做证据引用，不进估值计算。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Filing {
    pub form: String,
    pub filed_date: Option<String>,
    pub source_url: String,
}

/// 一条网页证据（Tavily 检索）——定性研究的二手来源，**只做定性支撑与引用，绝不进
/// `FactsRegistry`、绝不当作已核财务数字来源**（护栏仍只认一手财报）。缺字段用 `None`。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Evidence {
    pub title: String,
    pub url: String,
    /// 供应商返回的相关正文片段（已裁剪），供模型定性引用。
    pub snippet: String,
    pub published_date: Option<String>,
    /// 来源域名（如 `reuters.com`），便于标注与可信度判断。
    pub source_domain: Option<String>,
}

/// 内部人净买卖（Finnhub）——护栏只登金额与交易日。
#[derive(Clone, Debug, Default)]
pub struct InsiderActivity {
    pub net_value: Option<Decimal>,
    pub last_transaction_at: Option<String>,
}

/// 港股场内购回（HKEX 翌日披露）。
#[derive(Clone, Debug, Default)]
pub struct HkBuyback {
    pub total_consideration: Option<Decimal>,
    pub currency: Option<String>,
    pub trade_date: Option<String>,
}

/// 历史估值分位。
#[derive(Clone, Debug, Default)]
pub struct HistoricalValuation {
    pub percentile: Option<Decimal>,
    pub min: Option<Decimal>,
    pub max: Option<Decimal>,
    pub median: Option<Decimal>,
}

impl Financials {
    fn ok(&self) -> bool {
        self.provider_ok
    }
    /// EPS 可用于 PE 反推：为正且已年化。
    fn eps_usable_for_pe(&self) -> Option<Decimal> {
        match (self.eps, self.eps_annualized) {
            (Some(eps), ann) if eps > Decimal::ZERO && ann != Some(false) => Some(eps),
            _ => None,
        }
    }
}

/// 行情快照。
#[derive(Clone, Debug, Default)]
pub struct MarketSnapshot {
    pub price: Option<Decimal>,
    pub pe: Option<Decimal>,
    pub market_cap: Option<Decimal>,
    /// 报价币种（HKEX=HKD、美股=USD）。
    pub currency: Option<String>,
    pub change_percent: Option<Decimal>,
    pub dividend_yield: Option<Decimal>,
}

impl MarketSnapshot {
    /// 护栏语义的"providerStatus === ok"：拿到价格即视为已接地。
    #[must_use]
    pub fn is_ok(&self) -> bool {
        self.price.is_some()
    }
}

/// 公司档案里估值真正会用到的身份字段（不含任何数值兜底——见 research.ts 的注释：
/// 展示卡上的 "约 18x" 是字符串，不能当数字用）。
#[derive(Clone, Debug, Default)]
pub struct Company {
    pub sector: Option<String>,
    pub price: Option<Decimal>,
    pub pe: Option<Decimal>,
    pub pb: Option<Decimal>,
}

/// 资产阶段（spec §三）：决定用哪套估值口径。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum AssetStage {
    Profitable,
    LossGrowth,
    Loss,
    Unknown,
}

/// 单一方法推出的一条带子。
#[derive(Clone, Debug, Serialize)]
pub struct MethodBand {
    pub name: String,
    pub bear: Decimal,
    pub base: Decimal,
    pub bull: Decimal,
}

/// 估值结果——对齐 JS 版的返回形状，供提示词、前端估值条、factGuard 倍数登记表共用。
#[derive(Clone, Debug, Serialize)]
pub struct Valuation {
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bear: Option<Decimal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<Decimal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bull: Option<Decimal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upside: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub downside: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_price: Option<Decimal>,
    pub methods: Vec<String>,
    pub method_detail: Vec<MethodBand>,
    pub key_assumptions: Vec<String>,
    pub sensitivity: Vec<String>,
    #[serde(default)]
    pub stage_aware: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage: Option<AssetStage>,
    #[serde(default)]
    pub data_suspect: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cannot_value_reason: Option<String>,
}

impl Valuation {
    fn cannot(method: &str, reason: &str, assumptions: Vec<String>) -> Self {
        Self {
            method: method.to_string(),
            bear: None,
            base: None,
            bull: None,
            upside: None,
            downside: None,
            current_price: None,
            methods: Vec::new(),
            method_detail: Vec::new(),
            key_assumptions: assumptions,
            sensitivity: Vec::new(),
            stage_aware: false,
            stage: None,
            data_suspect: false,
            cannot_value_reason: Some(reason.to_string()),
        }
    }

    /// 是否给出了一条可信带子（downstream 判断 `dataSources.valuation` 是否 ok 的口径）。
    #[must_use]
    pub fn is_valued(&self) -> bool {
        self.cannot_value_reason.is_none() && self.base.is_some()
    }
}

fn clamp(x: Decimal, lo: Decimal, hi: Decimal) -> Decimal {
    x.max(lo).min(hi)
}

fn round2(x: Decimal) -> Decimal {
    x.round_dp(2)
}

fn pct_string(numerator: Decimal, price: Decimal) -> Option<String> {
    if price.is_zero() {
        return None;
    }
    let pct = (numerator / price) * Decimal::ONE_HUNDRED;
    Some(format!("{}%", pct.round_dp(1).normalize()))
}

/// 资产阶段分类（valuation.js `classifyAssetStage` 的忠实迁入）。
///
/// 经营利润率只在净利润/EPS 都缺失时才当亏损信号——阿里 26Q1 经营利润率 -0.05%（一次性
/// 费用噪音）、净利率 +9.7%，旧"任一为负即亏损"会把巨头误判成亏损高成长。
#[must_use]
pub fn classify_asset_stage(f: &Financials) -> AssetStage {
    if !f.ok() {
        return AssetStage::Unknown;
    }
    let net_loss = matches!(f.eps, Some(e) if e < Decimal::ZERO)
        || matches!(f.net_margin, Some(m) if m < Decimal::ZERO);
    let op_loss_only = f.eps.is_none()
        && f.net_margin.is_none()
        && matches!(f.operating_margin, Some(m) if m < Decimal::ZERO);
    if net_loss || op_loss_only {
        return match f.revenue_growth {
            Some(g) if g >= dec!(20) => AssetStage::LossGrowth,
            _ => AssetStage::Loss,
        };
    }
    AssetStage::Profitable
}

/// 亏损股 EV/Sales 情景估值（`computeEvSalesValuation` 迁入）。缺收入/股本返回 `None`（落回传统逻辑）。
fn compute_ev_sales(
    stage: AssetStage,
    market: &MarketSnapshot,
    f: &Financials,
    peer: Option<&PeerAnchor>,
) -> Option<Valuation> {
    // 企业价值在报价币种，收入/净现金在报告币种。没有 FX 换算时只允许同币种相除；
    // 例如腾讯报告币种 CNY、股价/市值 HKD，直接相除会生成伪 EV/Sales。
    if !matches!(
        (market.currency.as_deref(), f.currency.as_deref()),
        (Some(quote), Some(reporting)) if quote.eq_ignore_ascii_case(reporting)
    ) {
        return None;
    }
    let rev = f.revenue.filter(|r| *r > Decimal::ZERO)?;
    let shares = f.shares_outstanding.filter(|s| *s > Decimal::ZERO)?;
    let p = market.price.filter(|p| *p > Decimal::ZERO)?;

    // 净现金口径：优先用来源直接给出的净现金（港股一手抽取只给一个数），否则 现金-负债；
    // totalDebt 缺失（非 0）时不当净现金为 0。
    let net_cash = f.net_cash.unwrap_or_else(|| {
        f.cash_and_equivalents.unwrap_or(Decimal::ZERO) - f.total_debt.unwrap_or(Decimal::ZERO)
    });

    let growth = match f.revenue_growth {
        Some(g) => clamp(g / Decimal::ONE_HUNDRED, dec!(-0.5), dec!(1.5)),
        None => Decimal::ZERO,
    };
    let fwd_revenue = rev * (Decimal::ONE + growth);

    // 同业锚定优先；否则按增速分档的行业规则默认倍数。
    let use_peer = peer
        .is_some_and(|a| a.multiple_type == MultipleType::EvSales && a.n >= 2 && a.all_positive());
    let (t_bear, t_base, t_bull) = if let (true, Some(a)) = (use_peer, peer) {
        (a.p25, a.median, a.p75)
    } else if growth >= dec!(0.4) {
        (dec!(5), dec!(10), dec!(16))
    } else if growth >= dec!(0.2) {
        (dec!(3), dec!(6), dec!(10))
    } else {
        (dec!(1), dec!(2.5), dec!(4))
    };

    let price_at = |mult: Decimal| (mult * fwd_revenue + net_cash) / shares;
    let bear = price_at(t_bear);
    let base = price_at(t_base);
    let bull = price_at(t_bull);
    if !(bear > Decimal::ZERO && base > Decimal::ZERO && bull > Decimal::ZERO) {
        return None;
    }

    // 反推当前价隐含的 EV/Sales。
    let market_cap = market.market_cap.unwrap_or(p * shares);
    let ev = market_cap - net_cash;
    let implied_fwd = if fwd_revenue > Decimal::ZERO {
        Some(ev / fwd_revenue)
    } else {
        None
    };
    let implied_ttm = ev / rev;

    // 可信度护栏：带子必须与市场隐含倍数/现价大致同量级，否则判脏（SpaceX 复盘：市场 ~93x，
    // 引擎套 1–4x 吐出"该跌 94–98%"的脏带还挂"置信度高"）。
    let implied_ref = implied_fwd.unwrap_or(implied_ttm);
    let off_by_magnitude = implied_ref > t_bull * dec!(3) || implied_ref < t_bear / dec!(3);
    let band_disconnected = bull < p * dec!(0.5) || bear > p * dec!(2);
    let shares_vs_cap = match market.market_cap {
        Some(mc) if mc > Decimal::ZERO => ((p * shares - mc).abs() / mc) > dec!(0.5),
        _ => false,
    };
    if off_by_magnitude || band_disconnected || shares_vs_cap {
        return Some(Valuation {
            method: "EV/Sales 情景".into(),
            bear: None,
            base: None,
            bull: None,
            upside: None,
            downside: None,
            current_price: Some(p),
            methods: Vec::new(),
            method_detail: Vec::new(),
            key_assumptions: Vec::new(),
            sensitivity: Vec::new(),
            stage_aware: true,
            stage: Some(stage),
            data_suspect: true,
            cannot_value_reason: Some(
                "财报数据与市场定价严重不符（疑似新上市/数据缺口），暂不给可信估值。".into(),
            ),
        });
    }

    let stage_txt = if stage == AssetStage::LossGrowth {
        "亏损高成长"
    } else {
        "亏损"
    };
    Some(Valuation {
        method: "EV/Sales 情景".into(),
        bear: Some(round2(bear)),
        base: Some(round2(base)),
        bull: Some(round2(bull)),
        upside: pct_string(base - p, p),
        downside: pct_string(bear - p, p),
        current_price: Some(p),
        methods: vec!["EV/Sales 情景".into()],
        method_detail: vec![MethodBand {
            name: "EV/Sales".into(),
            bear: round2(bear),
            base: round2(base),
            bull: round2(bull),
        }],
        key_assumptions: vec![
            format!("阶段：{stage_txt}（利润为负、PE 不适用）→ 用 EV/Sales 情景，不套 PE 带"),
            if use_peer {
                format!(
                    "看空 {}x / 中性 {}x / 看多 {}x EV/Sales（同业 {} 家可比的分位数）",
                    t_bear.normalize(),
                    t_base.normalize(),
                    t_bull.normalize(),
                    peer.map_or(0, |a| a.n)
                )
            } else {
                format!(
                    "看空 {}x / 中性 {}x / 看多 {}x EV/Sales（同业数据不足，行业规则默认倍数）",
                    t_bear.normalize(),
                    t_base.normalize(),
                    t_bull.normalize()
                )
            },
        ],
        sensitivity: vec![format!(
            "EV/Sales 每变化 1x，目标价变化约 {}",
            round2(fwd_revenue / shares)
        )],
        stage_aware: true,
        stage: Some(stage),
        data_suspect: false,
        cannot_value_reason: None,
    })
}

struct WeightedMethod {
    name: String,
    bear: Decimal,
    base: Decimal,
    bull: Decimal,
}

/// 多方法估值（`computeValuation` 迁入）。所有方法等权，因此加权平均即算术平均。
#[must_use]
pub fn compute_valuation(
    company: &Company,
    market: &MarketSnapshot,
    f: &Financials,
    peer: Option<&PeerAnchor>,
) -> Valuation {
    let price = match market.price.or(company.price) {
        Some(p) => p,
        None => return Valuation::cannot("无法估值", "缺行情价格，无法估值。", Vec::new()),
    };
    let pe = market.pe.or(company.pe);
    let market_cap = market.market_cap;
    let has_financials = f.ok();

    // ── Stage-aware：亏损股走 EV/Sales，避免掉进"以现价为中心的 PE 带"自循环。
    let stage = classify_asset_stage(f);
    if matches!(stage, AssetStage::LossGrowth | AssetStage::Loss) && has_financials {
        if let Some(ev) = compute_ev_sales(stage, market, f, peer) {
            return ev;
        }
    }

    let mut methods: Vec<WeightedMethod> = Vec::new();
    let mut assumptions: Vec<String> = Vec::new();
    let mut sensitivity: Vec<String> = Vec::new();

    // ── PE 法：pe>0 && eps>0 && 已年化。
    if let (Some(pe), Some(eps)) = (pe.filter(|p| *p > Decimal::ZERO), f.eps_usable_for_pe()) {
        let (pe_bear, pe_base, pe_bull) = (pe * dec!(0.7), pe, pe * dec!(1.3));
        methods.push(WeightedMethod {
            name: "PE".into(),
            bear: eps * pe_bear,
            base: eps * pe_base,
            bull: eps * pe_bull,
        });
        assumptions.push(format!(
            "Trailing PE {}x（bear {}x / base {}x / bull {}x）",
            pe.normalize(),
            round2(pe_bear).normalize(),
            round2(pe_base).normalize(),
            round2(pe_bull).normalize()
        ));
        assumptions.push(format!("EPS {}", eps.normalize()));
        sensitivity.push(format!("PE 每变化 1x，目标价变化约 {}", eps.normalize()));
    }

    // ── 同业倍数 PE 法：用同业 PE 分位数 × 自身 EPS。
    if let (Some(a), Some(eps)) = (
        peer.filter(|a| a.multiple_type == MultipleType::Pe && a.n >= 2 && a.all_positive()),
        f.eps_usable_for_pe(),
    ) {
        methods.push(WeightedMethod {
            name: "同业倍数 PE".into(),
            bear: eps * a.p25,
            base: eps * a.median,
            bull: eps * a.p75,
        });
        assumptions.push(format!(
            "同业倍数 PE：{} 家同阶段可比（{}）的 PE p25 {}x / 中位 {}x / p75 {}x × 自身 EPS {}",
            a.n,
            a.tickers.join("、"),
            a.p25.normalize(),
            a.median.normalize(),
            a.p75.normalize(),
            eps.normalize()
        ));
    }

    // ── Forward PE 法。
    if let (Some(fwd_pe), Some(eps)) = (
        f.forward_pe.filter(|p| *p > Decimal::ZERO),
        f.eps_usable_for_pe(),
    ) {
        methods.push(WeightedMethod {
            name: "Forward PE".into(),
            bear: eps * (fwd_pe * dec!(0.7)),
            base: eps * fwd_pe,
            bull: eps * (fwd_pe * dec!(1.3)),
        });
        assumptions.push(format!("Forward PE {}x", fwd_pe.normalize()));
    }

    // ── FCF Yield 法（bear 10% / base 7% / bull 5%）。
    if let (Some(fcf), Some(shares)) = (
        f.free_cash_flow,
        f.shares_outstanding.filter(|s| *s > Decimal::ZERO),
    ) {
        let fcf_ps = fcf / shares;
        methods.push(WeightedMethod {
            name: "FCF Yield".into(),
            bear: fcf_ps / dec!(0.10),
            base: fcf_ps / dec!(0.07),
            bull: fcf_ps / dec!(0.05),
        });
        assumptions.push(format!(
            "FCF per share {}（收益率 5–10%）",
            round2(fcf_ps).normalize()
        ));
    }

    // ── DCF 简化（5 年 + 永续），净现金口径同 EV/Sales。
    if let (Some(fcf0), Some(growth_pct)) = (f.free_cash_flow, f.revenue_growth) {
        let growth = growth_pct / Decimal::ONE_HUNDRED;
        let wacc = dec!(0.10);
        let terminal_growth = dec!(0.03);
        let mut pv = Decimal::ZERO;
        let mut fcf = fcf0;
        let mut discount = Decimal::ONE;
        for _ in 1..=5 {
            fcf *= Decimal::ONE + growth;
            discount *= Decimal::ONE + wacc;
            pv += fcf / discount;
        }
        let terminal = fcf * (Decimal::ONE + terminal_growth) / (wacc - terminal_growth) / discount;
        let enterprise_value = pv + terminal;
        let net_cash = f.net_cash.unwrap_or_else(|| {
            f.cash_and_equivalents.unwrap_or(Decimal::ZERO) - f.total_debt.unwrap_or(Decimal::ZERO)
        });
        let equity_value = enterprise_value + net_cash;
        let shares_out = f
            .shares_outstanding
            .or_else(|| market_cap.map(|mc| mc / price));
        if let Some(shares_out) = shares_out.filter(|s| *s > Decimal::ZERO) {
            let dcf = equity_value / shares_out;
            methods.push(WeightedMethod {
                name: "DCF".into(),
                bear: dcf * dec!(0.8),
                base: if dcf.is_zero() { price } else { dcf },
                bull: dcf * dec!(1.2),
            });
            assumptions.push(format!(
                "DCF：WACC 10%，永续增长 3%，净现金 {}",
                round2(net_cash).normalize()
            ));
        }
    }

    if methods.is_empty() {
        // 关键不变量：绝不回退到"以现价为中心的 PE 带"。缺口径就诚实不给带子。
        return Valuation::cannot(
            "无法估值",
            "缺少可信的 PE、自由现金流与 EPS 口径，本轮不给估值区间。",
            vec!["缺少 PE、EPS、FCF 等估值所需数据。".into()],
        );
    }

    let n = Decimal::from(methods.len() as u64);
    let mean = |sel: fn(&WeightedMethod) -> Decimal| methods.iter().map(sel).sum::<Decimal>() / n;
    let (bear, base, bull) = (mean(|m| m.bear), mean(|m| m.base), mean(|m| m.bull));

    Valuation {
        method: methods
            .iter()
            .map(|m| m.name.clone())
            .collect::<Vec<_>>()
            .join(" + "),
        bear: Some(round2(bear)),
        base: Some(round2(base)),
        bull: Some(round2(bull)),
        upside: pct_string(base - price, price),
        downside: pct_string(bear - price, price),
        current_price: Some(price),
        methods: methods.iter().map(|m| m.name.clone()).collect(),
        method_detail: methods
            .iter()
            .map(|m| MethodBand {
                name: m.name.clone(),
                bear: round2(m.bear),
                base: round2(m.base),
                bull: round2(m.bull),
            })
            .collect(),
        key_assumptions: assumptions,
        sensitivity,
        stage_aware: false,
        stage: Some(stage),
        data_suspect: false,
        cannot_value_reason: None,
    }
}

/// 展示安全估值（`displayValuation` 迁入）。分析师目标价源尚未接通（估值 estimates 恒为 None），
/// 因此两条兜底路径都不会产出 analyst band——与当前 research.ts 的接线一致。
///
/// 关键不变量：缺口径时**不回退到以现价为中心的 PE 带**（自循环根因），而是诚实标注缺口径。
/// 这条防线在 [`compute_valuation`]（`methods.is_empty()` → `cannot`），不在本函数。
///
/// 本函数只判"带子与现价是否脱节"，判据见 [`band_coherent`]：**不要求带子跨越现价**。
/// 整条带落在现价下方就是"这股票贵"、落在上方就是"便宜"，正是用户问"贵不贵"时最想要的结论；
/// 旧口径要求 `bear < price < bull`，等于把所有强观点都当成数据错误丢掉（实测 MSFT、INTC
/// 四法都算得出带子，全被这一条吞掉）。自循环的特征是带子**以现价为中心**，不是带子**包含现价**，
/// 用后者当代理指标是错的。亏损股的 EV/Sales 路径早已豁免此检查，这里与之统一口径。
#[must_use]
pub fn display_valuation(
    company: &Company,
    market: &MarketSnapshot,
    f: &Financials,
    peer: Option<&PeerAnchor>,
) -> Valuation {
    let v = compute_valuation(company, market, f, peer);

    // stage-aware 脏数据：只允许回退到可信分析师带（当前无源 → 返回原判罚）。
    if v.stage_aware && v.data_suspect {
        return v;
    }
    // stage-aware 情景带刻意允许整条带在现价上/下方，不套自洽检查。
    if v.stage_aware && v.cannot_value_reason.is_none() {
        return v;
    }

    let price = market.price.or(company.price);
    let coherent = match (v.bear, v.base, v.bull, price) {
        (Some(bear), Some(base), Some(bull), Some(price)) if v.cannot_value_reason.is_none() => {
            band_coherent(bear, base, bull, price)
        }
        _ => false,
    };
    if coherent {
        return v;
    }

    // 无可信自洽口径 → 诚实不给带子（绝不掉进"现价 ×0.78/1.0/1.28"）。
    Valuation::cannot(
        &v.method,
        v.cannot_value_reason
            .as_deref()
            .unwrap_or("缺少自洽的估值口径，本轮不给估值区间。"),
        Vec::new(),
    )
}

/// 带子是否可信：**内部有序** + **与现价同量级**。允许整条带落在现价任一侧。
///
/// 同量级阈值与 `compute_ev_sales` 的 `band_disconnected` 同源（`bull < 现价/2` 或
/// `bear > 现价×2` 即判脱节），全产品对"带子与现价脱节"只保留一个定义。含义是：模型最多
/// 说得出"该跌一半"或"该涨一倍"，再离谱就属于数据脏（错配股本、错配币种、错配报告期），
/// 不是强观点——SpaceX 复盘里那条"该跌 94–98%"还挂着高置信度的脏带正是这样来的。
fn band_coherent(bear: Decimal, base: Decimal, bull: Decimal, price: Decimal) -> bool {
    let ordered = bear > Decimal::ZERO && bear <= base && base <= bull;
    let same_magnitude = bull >= price / dec!(2) && bear <= price * dec!(2);
    ordered && same_magnitude
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profitable_financials() -> Financials {
        Financials {
            provider_ok: true,
            eps: Some(dec!(6.0)),
            net_margin: Some(dec!(30.4)),
            revenue: Some(dec!(650_000_000_000)),
            revenue_growth: Some(dec!(11.0)),
            shares_outstanding: Some(dec!(9_200_000_000)),
            ..Default::default()
        }
    }

    #[test]
    fn profitable_pe_band_brackets_price() {
        let company = Company {
            sector: Some("互联网".into()),
            ..Default::default()
        };
        // 现价 120 落在 PE 隐含带内（bear 75.6 / base 108 / bull 140.4）——最常见的形态。
        let market = MarketSnapshot {
            price: Some(dec!(120)),
            pe: Some(dec!(18)),
            market_cap: Some(dec!(1_100_000_000_000)),
            ..Default::default()
        };
        let v = display_valuation(&company, &market, &profitable_financials(), None);
        assert!(
            v.is_valued(),
            "profitable PE band should produce a valuation: {v:?}"
        );
        // base = eps 6 × pe 18 = 108
        assert_eq!(v.base.unwrap(), dec!(108.00));
    }

    /// 整条带落在现价下方 = "这股票贵"，是有效研究结论，不是数据错误。旧口径要求
    /// `bear < 现价 < bull`，把这类强观点全判成"不自洽"吞掉（MSFT/INTC 实测即如此）。
    #[test]
    fn band_entirely_below_price_still_counts_as_expensive() {
        let company = Company {
            sector: Some("互联网".into()),
            ..Default::default()
        };
        // PE 隐含带 bear 75.6 / base 108 / bull 140.4，现价 200 高于整条带。
        let market = MarketSnapshot {
            price: Some(dec!(200)),
            pe: Some(dec!(18)),
            market_cap: Some(dec!(1_100_000_000_000)),
            ..Default::default()
        };
        let v = display_valuation(&company, &market, &profitable_financials(), None);
        assert!(v.is_valued(), "整条带在现价下方应给出结论: {v:?}");
        assert_eq!(v.base.unwrap(), dec!(108.00));
        // 下行空间为负——这正是"贵"的量化表达。
        assert!(v.upside.as_deref().is_some_and(|u| u.starts_with('-')));
    }

    /// 但脱节仍要拦：带子比现价低一个数量级，属于口径错配（错股本/错币种/错报告期），
    /// 不是"贵得离谱"。阈值与 EV/Sales 路径同源。
    #[test]
    fn band_an_order_of_magnitude_off_is_still_refused() {
        let company = Company {
            sector: Some("互联网".into()),
            ..Default::default()
        };
        // bull 140.4 远低于现价一半（1500/2 = 750）→ 判脱节。
        let market = MarketSnapshot {
            price: Some(dec!(1500)),
            pe: Some(dec!(18)),
            market_cap: Some(dec!(1_100_000_000_000)),
            ..Default::default()
        };
        let v = display_valuation(&company, &market, &profitable_financials(), None);
        assert!(!v.is_valued());
        assert!(v.cannot_value_reason.is_some());
    }

    #[test]
    fn band_coherence_allows_either_side_but_not_disconnection() {
        // 内部有序 + 同量级：通过（带子整体在现价上方 = 便宜）。
        assert!(band_coherent(dec!(120), dec!(150), dec!(180), dec!(100)));
        // 整体在现价下方 = 贵：同样通过。
        assert!(band_coherent(dec!(60), dec!(75), dec!(90), dec!(100)));
        // 边界：bull 恰为现价一半 / bear 恰为现价两倍，仍算同量级。
        assert!(band_coherent(dec!(30), dec!(40), dec!(50), dec!(100)));
        assert!(band_coherent(dec!(200), dec!(240), dec!(280), dec!(100)));
        // 脱节：bull 不足现价一半 / bear 超过现价两倍。
        assert!(!band_coherent(dec!(20), dec!(30), dec!(49), dec!(100)));
        assert!(!band_coherent(dec!(201), dec!(240), dec!(280), dec!(100)));
        // 内部失序或非正：拒绝。
        assert!(!band_coherent(dec!(90), dec!(75), dec!(60), dec!(100)));
        assert!(!band_coherent(dec!(0), dec!(75), dec!(90), dec!(100)));
    }

    #[test]
    fn loss_making_never_uses_negative_pe() {
        // 负 EPS × 负 PE 的"负负得正"必须被拦截 —— 亏损股不走 PE 法。
        let company = Company::default();
        let market = MarketSnapshot {
            price: Some(dec!(50)),
            pe: Some(dec!(-78)),
            market_cap: Some(dec!(20_000_000_000)),
            currency: Some("HKD".into()),
            ..Default::default()
        };
        let f = Financials {
            provider_ok: true,
            eps: Some(dec!(-3.0)),
            net_margin: Some(dec!(-12.0)),
            revenue: Some(dec!(4_000_000_000)),
            revenue_growth: Some(dec!(35.0)),
            shares_outstanding: Some(dec!(400_000_000)),
            currency: Some("HKD".into()),
            ..Default::default()
        };
        let v = display_valuation(&company, &market, &f, None);
        assert_eq!(classify_asset_stage(&f), AssetStage::LossGrowth);
        assert!(
            v.stage_aware,
            "loss-making must go stage-aware EV/Sales, not PE"
        );
        // 无论最终是否给带子，method 都不能是负负得正的 PE。
        assert!(!v.method.contains("PE") || v.stage_aware);
    }

    #[test]
    fn ev_sales_refuses_cross_currency_amounts_without_fx() {
        let market = MarketSnapshot {
            price: Some(dec!(50)),
            market_cap: Some(dec!(20_000_000_000)),
            currency: Some("HKD".into()),
            ..Default::default()
        };
        let f = Financials {
            provider_ok: true,
            eps: Some(dec!(-3)),
            net_margin: Some(dec!(-12)),
            revenue: Some(dec!(4_000_000_000)),
            shares_outstanding: Some(dec!(400_000_000)),
            currency: Some("CNY".into()),
            ..Default::default()
        };
        let valuation = display_valuation(&Company::default(), &market, &f, None);
        assert!(!valuation.is_valued());
        assert!(!valuation.stage_aware);
    }

    #[test]
    fn missing_pe_and_eps_refuses_a_band_instead_of_price_centered_pe() {
        // 港股旗舰的复发场景：无 pe_ttm、无可用 EPS → 绝不产出"现价 ×0.78/1.0/1.28"零信息带。
        let company = Company::default();
        let market = MarketSnapshot {
            price: Some(dec!(400)),
            pe: None,
            market_cap: Some(dec!(3_600_000_000_000)),
            ..Default::default()
        };
        let f = Financials {
            provider_ok: true,
            net_margin: Some(dec!(28.0)),
            ..Default::default()
        };
        let v = display_valuation(&company, &market, &f, None);
        assert!(!v.is_valued());
        assert!(v.cannot_value_reason.is_some());
    }

    #[test]
    fn interim_cumulative_eps_is_not_used_for_pe() {
        // eps_annualized == Some(false)：报告期累计 EPS 不得反推 PE。
        let company = Company::default();
        let market = MarketSnapshot {
            price: Some(dec!(100)),
            pe: Some(dec!(15)),
            market_cap: Some(dec!(100_000_000_000)),
            ..Default::default()
        };
        let f = Financials {
            provider_ok: true,
            eps: Some(dec!(2.0)),
            eps_annualized: Some(false),
            net_margin: Some(dec!(20.0)),
            ..Default::default()
        };
        let v = display_valuation(&company, &market, &f, None);
        // 没有其它方法可用 → 不给带子，而不是用累计 EPS 拼一条。
        assert!(!v.is_valued());
    }
}
