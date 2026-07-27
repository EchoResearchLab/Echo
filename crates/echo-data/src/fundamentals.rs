//! FMP `stable` 美股三表 fundamentals。
//!
//! 边界与旧 `fmpFundamentalsAdapter` 对齐：免费档三表只对美股代码可用；HK/CN 会
//! 返回 premium 错误体，因此 `supports` 严格 US-only。商用模式禁止未授权免费源。
//! 季度 EPS 不得用于反推 PE——调用方应优先使用本结果的 `eps_ttm` / `pe_ttm`。
//!
//! **TTM 口径来自 `ratios-ttm`**（免费档已覆盖）：过去只从这个响应里取了 `pe_ttm` 一个字段，
//! 而估值方法全都要年化 EPS，于是 `compute_valuation` 在生产链路上 100% 落到 `cannot_value`——
//! 估值逻辑与全部不变量测试都绿，却从未产出过一次估值区间。同一个响应里本来就有
//! `netIncomePerShareTTM`（年化 EPS），取回来即让同业倍数 PE 点火，不增加任何一次外部请求。
//! `free_cash_flow_ttm` 现在不进估值（简化 DCF 与 FCF Yield 已移除），但仍进护栏登记表与
//! 事实块——模型引用现金流数字时要有据可核。

use crate::fmp::{self, FmpError, decimal_at, fetch_json, string_at};
use crate::{Market, detect_market, normalize_ticker};
use echo_config::DataSourceConfig;
use rust_decimal::Decimal;
use serde_json::Value;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FundamentalsRow {
    pub currency: Option<String>,
    pub revenue: Option<Decimal>,
    pub gross_profit: Option<Decimal>,
    pub operating_income: Option<Decimal>,
    pub net_income: Option<Decimal>,
    pub operating_cash_flow: Option<Decimal>,
    pub cash_and_equivalents: Option<Decimal>,
    pub net_cash: Option<Decimal>,
    /// 单季 EPS，仅供展示；估值应用 `eps_ttm`，并把 `eps_annualized` 视为 false。
    pub eps: Option<Decimal>,
    /// TTM 每股收益（`netIncomePerShareTTM`）——已年化，是估值 PE 法唯一可用的 EPS 口径。
    /// 亏损公司为负，如实透传：由 `classify_asset_stage` 决定改走 EV/Sales，不在这里过滤。
    pub eps_ttm: Option<Decimal>,
    pub pe_ttm: Option<Decimal>,
    /// TTM 自由现金流总额 = `freeCashFlowPerShareTTM` × 摊薄股本。供应商只给每股口径，
    /// 乘回总额是为了让 DCF 与 EV/Sales 用同一个绝对值基数；缺任一乘数即 `None`。
    pub free_cash_flow_ttm: Option<Decimal>,
    /// 摊薄股本（`weightedAverageShsOutDil`，缺失退回基本股本）。
    pub shares_outstanding: Option<Decimal>,
    pub total_debt: Option<Decimal>,
    pub revenue_prior: Option<Decimal>,
    pub net_income_prior: Option<Decimal>,
    pub period_end: Option<String>,
    pub published_at: Option<String>,
    pub period_label: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FundamentalsResult {
    pub provider_ok: bool,
    pub source: String,
    pub rows: Vec<FundamentalsRow>,
}

impl FundamentalsResult {
    #[must_use]
    pub fn missing(source: impl Into<String>) -> Self {
        Self {
            provider_ok: false,
            source: source.into(),
            rows: Vec::new(),
        }
    }

    #[must_use]
    pub fn latest(&self) -> Option<&FundamentalsRow> {
        self.rows.first()
    }
}

pub type FundamentalsError = FmpError;

#[derive(Clone)]
pub struct FundamentalsService {
    client: reqwest::Client,
    config: DataSourceConfig,
}

impl FundamentalsService {
    pub fn new(config: DataSourceConfig) -> Result<Self, FundamentalsError> {
        Ok(Self {
            client: fmp::build_client()?,
            config,
        })
    }

    /// 取最新可用美股财报行。未配 key / 非美股 / 商用模式 / 上游失败 → `missing`，不抛给主链路。
    pub async fn fetch(&self, raw_ticker: &str) -> FundamentalsResult {
        match self.fetch_strict(raw_ticker).await {
            Ok(result) => result,
            Err(error) => {
                tracing::warn!(ticker = raw_ticker, error = %error, "FMP fundamentals 未核到");
                FundamentalsResult::missing("FMP")
            }
        }
    }

    async fn fetch_strict(
        &self,
        raw_ticker: &str,
    ) -> Result<FundamentalsResult, FundamentalsError> {
        if self.config.commercial_mode {
            return Err(FundamentalsError::CommercialBlocked);
        }
        let Some(api_key) = self.config.fmp_api_key.as_deref() else {
            return Err(FundamentalsError::MissingApiKey);
        };
        let ticker = normalize_ticker(raw_ticker);
        if detect_market(&ticker) != Market::Us {
            return Err(FundamentalsError::UnsupportedMarket(ticker));
        }

        // limit=5 才够拿到「去年同期」（索引 4）。免费档 limit 上限恰好是 5。
        let income_path = format!(
            "income-statement?symbol={}&period=quarter&limit=5",
            encode(&ticker)
        );
        let cash_path = format!(
            "cash-flow-statement?symbol={}&period=quarter&limit=1",
            encode(&ticker)
        );
        let balance_path = format!(
            "balance-sheet-statement?symbol={}&period=quarter&limit=1",
            encode(&ticker)
        );
        let ratios_path = format!("ratios-ttm?symbol={}", encode(&ticker));

        let (income, cash_flow, balance_sheet, ratios_ttm) = tokio::join!(
            fetch_json(&self.client, api_key, &income_path),
            fetch_json(&self.client, api_key, &cash_path),
            fetch_json(&self.client, api_key, &balance_path),
            fetch_json(&self.client, api_key, &ratios_path),
        );

        let income = income?;
        let cash = cash_flow.ok();
        let balance = balance_sheet.ok();
        let ratios = ratios_ttm.ok();

        let current = first_object(&income);
        // 同比基期 = 四个季度前，**不是上一季**。季度环比对有季节性的公司是纯噪声：
        // 苹果 FY26Q2 比 FY26Q1（假日季）营收 -22.7%、利润 -29.7%，同比却是 +16.6% / +19.4%。
        // 这两个增速既进 DCF 的复合增长，又进作答事实块——环比口径下模型会被告知"苹果营收
        // 下滑 22.7%"，而数字护栏只核"答案与事实块一致"，对口径错误无感，照样判全过。
        // 上市不足五个季度 → `None` → 增速缺数，不拿环比顶替。
        let prior = nth_object(&income, 4);
        let Some(current) = current else {
            return Ok(FundamentalsResult::missing("FMP"));
        };
        let cash_row = cash.as_ref().and_then(first_object);
        let balance_row = balance.as_ref().and_then(first_object);
        let ratios_row = ratios.as_ref().and_then(first_object);

        Ok(FundamentalsResult {
            provider_ok: true,
            source: "FMP".into(),
            rows: vec![map_row(current, prior, cash_row, balance_row, ratios_row)],
        })
    }
}

/// 四个 FMP 响应 → 一行事实。独立于 HTTP，测试直接喂 fixture 走同一条映射——此前测试把
/// 映射逻辑抄了一份，改了取数字段测试也照样绿，正是估值腿断了一直没被发现的原因之一。
fn map_row(
    current: &Value,
    prior: Option<&Value>,
    cash_row: Option<&Value>,
    balance_row: Option<&Value>,
    ratios_row: Option<&Value>,
) -> FundamentalsRow {
    let pe_ttm = ratios_row
        .and_then(|row| decimal_at(row, "priceToEarningsRatioTTM"))
        .filter(|value| *value > Decimal::ZERO);
    let eps_ttm = ratios_row.and_then(|row| decimal_at(row, "netIncomePerShareTTM"));

    // 摊薄优先：回购在摊薄口径下才反映到每股，基本股本只作兜底。
    let shares_outstanding = decimal_at(current, "weightedAverageShsOutDil")
        .or_else(|| decimal_at(current, "weightedAverageShsOut"))
        .filter(|value| *value > Decimal::ZERO);
    let free_cash_flow_ttm = ratios_row
        .and_then(|row| decimal_at(row, "freeCashFlowPerShareTTM"))
        .zip(shares_outstanding)
        .map(|(per_share, shares)| per_share * shares);

    let period_label = match (
        string_at(current, "fiscalYear"),
        string_at(current, "period"),
    ) {
        (Some(year), Some(period)) => Some(format!("{year} {period}")),
        _ => None,
    };
    let net_debt = balance_row.and_then(|row| decimal_at(row, "netDebt"));

    FundamentalsRow {
        currency: string_at(current, "reportedCurrency"),
        revenue: decimal_at(current, "revenue"),
        gross_profit: decimal_at(current, "grossProfit"),
        operating_income: decimal_at(current, "operatingIncome"),
        net_income: decimal_at(current, "netIncome"),
        operating_cash_flow: cash_row
            .and_then(|row| decimal_at(row, "netCashProvidedByOperatingActivities")),
        cash_and_equivalents: balance_row.and_then(|row| decimal_at(row, "cashAndCashEquivalents")),
        net_cash: net_debt.map(|debt| -debt),
        eps: decimal_at(current, "epsDiluted").or_else(|| decimal_at(current, "eps")),
        eps_ttm,
        pe_ttm,
        free_cash_flow_ttm,
        shares_outstanding,
        total_debt: balance_row.and_then(|row| decimal_at(row, "totalDebt")),
        revenue_prior: prior.and_then(|row| decimal_at(row, "revenue")),
        net_income_prior: prior.and_then(|row| decimal_at(row, "netIncome")),
        period_end: string_at(current, "date"),
        published_at: string_at(current, "filingDate"),
        period_label,
    }
}

fn encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

fn first_object(body: &Value) -> Option<&Value> {
    nth_object(body, 0)
}

fn nth_object(body: &Value, index: usize) -> Option<&Value> {
    body.as_array()?.get(index).filter(|item| item.is_object())
}

/// 把 FMP 行映射成估值/护栏可用的派生字段（同比%、利润率）。
#[must_use]
pub fn pct_of(part: Option<Decimal>, whole: Option<Decimal>) -> Option<Decimal> {
    match (part, whole) {
        (Some(part), Some(whole)) if !whole.is_zero() => Some(part * Decimal::from(100) / whole),
        _ => None,
    }
}

#[must_use]
pub fn pct_change(current: Option<Decimal>, prior: Option<Decimal>) -> Option<Decimal> {
    match (current, prior) {
        (Some(current), Some(prior)) if !prior.is_zero() => {
            Some((current - prior) * Decimal::from(100) / prior)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;
    use serde_json::json;

    /// 字段名与五个季度的数值取自 2026-07-27 对 `stable` 免费档 AAPL 的实测响应，勿凭印象改。
    /// 序列刻意保留真实的季节性：Q1 是假日季（1438 亿），环比会给出 -22.7% 的假下滑。
    fn aapl_fixture() -> (Value, Value, Value, Value) {
        let income = json!([
            {
                "date": "2026-03-28",
                "filingDate": "2026-05-01",
                "reportedCurrency": "USD",
                "fiscalYear": "2026",
                "period": "Q2",
                "revenue": 111_226_000_000i64,
                "grossProfit": 51_000_000_000i64,
                "operatingIncome": 34_000_000_000i64,
                "netIncome": 29_600_000_000i64,
                "eps": 2.02,
                "epsDiluted": 2.01,
                "weightedAverageShsOut": 14_710_718_000i64,
                "weightedAverageShsOutDil": 14_768_115_000i64
            },
            { "period": "Q1", "revenue": 143_800_000_000i64, "netIncome": 42_100_000_000i64 },
            { "period": "Q4", "revenue": 102_500_000_000i64, "netIncome": 27_500_000_000i64 },
            { "period": "Q3", "revenue":  94_000_000_000i64, "netIncome": 23_400_000_000i64 },
            { "period": "Q2", "revenue":  95_359_000_000i64, "netIncome": 24_780_000_000i64 }
        ]);
        let cash = json!([{
            "netCashProvidedByOperatingActivities": 28_702_000_000i64,
            "freeCashFlow": 26_731_000_000i64
        }]);
        let balance = json!([{
            "cashAndCashEquivalents": 30_000_000_000i64,
            "totalDebt": 98_000_000_000i64,
            "netDebt": -10_000_000_000i64
        }]);
        let ratios = json!([{
            "priceToEarningsRatioTTM": 40.17129071170084,
            "netIncomePerShareTTM": 8.332360120015895,
            "freeCashFlowPerShareTTM": 8.780944614668027
        }]);
        (income, cash, balance, ratios)
    }

    fn map_fixture() -> FundamentalsRow {
        let (income, cash, balance, ratios) = aapl_fixture();
        map_row(
            first_object(&income).expect("current"),
            nth_object(&income, 4),
            first_object(&cash),
            first_object(&balance),
            first_object(&ratios),
        )
    }

    #[test]
    fn maps_stable_fixture_like_retired_adapter() {
        let row = map_fixture();
        assert_eq!(row.eps, Some(dec!(2.01)));
        assert_eq!(row.pe_ttm, Some(dec!(40.17129071170084)));
        assert_eq!(row.net_cash, Some(dec!(10000000000)));
        assert_eq!(row.period_label.as_deref(), Some("2026 Q2"));
        let margin = pct_of(row.net_income, row.revenue).expect("margin");
        assert!(margin > dec!(25) && margin < dec!(27));
    }

    /// 增速必须是同比（对去年同期），不是环比。环比在苹果这种假日季公司上会把 +16.6% 的
    /// 增长读成 -22.7% 的下滑，既污染作答事实块，又被 DCF 当成年增长率复合五年
    /// （实测把每股 DCF 从约 146 美元压到 36 美元）。
    #[test]
    fn growth_is_year_over_year_not_sequential() {
        let row = map_fixture();
        let revenue_growth = pct_change(row.revenue, row.revenue_prior).expect("营收增速");
        let profit_growth = pct_change(row.net_income, row.net_income_prior).expect("利润增速");
        assert!(
            revenue_growth > dec!(16) && revenue_growth < dec!(17),
            "同比应约 +16.6%，实际 {revenue_growth}"
        );
        assert!(
            profit_growth > dec!(19) && profit_growth < dec!(20),
            "同比应约 +19.4%，实际 {profit_growth}"
        );
    }

    /// 上市不足五个季度：增速诚实缺数，绝不退回环比顶替。
    #[test]
    fn young_listing_has_no_growth_rather_than_sequential() {
        let (income, cash, balance, ratios) = aapl_fixture();
        let short = json!([income.as_array().expect("array")[0].clone()]);
        let row = map_row(
            first_object(&short).expect("current"),
            nth_object(&short, 4),
            first_object(&cash),
            first_object(&balance),
            first_object(&ratios),
        );
        assert_eq!(row.revenue_prior, None);
        assert_eq!(pct_change(row.revenue, row.revenue_prior), None);
    }

    /// 估值四法（PE / 同业 PE / FCF Yield / DCF）所依赖的口径必须全部到位——这些字段任一
    /// 回到 `None`，`compute_valuation` 就会重新落到 `cannot_value`，估值区间再次消失。
    #[test]
    fn ttm_fields_unlock_valuation() {
        let row = map_fixture();
        assert_eq!(
            row.eps_ttm,
            Some(dec!(8.332360120015895)),
            "PE 法要年化 EPS"
        );
        // 摊薄优先，不取 weightedAverageShsOut。
        assert_eq!(row.shares_outstanding, Some(dec!(14768115000)));
        // 每股 FCF × 摊薄股本 ≈ 1297 亿美元。
        let fcf = row.free_cash_flow_ttm.expect("FCF Yield 与 DCF 要总额 FCF");
        assert!(
            fcf > dec!(128_000_000_000) && fcf < dec!(131_000_000_000),
            "FCF={fcf}"
        );
        assert_eq!(row.total_debt, Some(dec!(98000000000)));
    }

    /// 免费档偶尔整份 `ratios-ttm` 取不到（限流/字段缺失）。此时估值该诚实失败，
    /// 绝不能用单季 EPS 冒充年化——那会算出四倍虚高的目标价。
    #[test]
    fn missing_ratios_leaves_ttm_fields_none() {
        let (income, cash, balance, _) = aapl_fixture();
        let row = map_row(
            first_object(&income).expect("current"),
            None,
            first_object(&cash),
            first_object(&balance),
            None,
        );
        assert_eq!(row.eps_ttm, None);
        assert_eq!(row.pe_ttm, None);
        assert_eq!(row.free_cash_flow_ttm, None);
        // 股本来自 income-statement，不受 ratios 缺失影响。
        assert_eq!(row.shares_outstanding, Some(dec!(14768115000)));
    }

    #[tokio::test]
    async fn commercial_mode_and_hk_never_call_out() {
        let commercial = FundamentalsService::new(DataSourceConfig {
            commercial_mode: true,
            fmp_api_key: Some("x".into()),
            ..Default::default()
        })
        .expect("service");
        assert!(!commercial.fetch("AAPL").await.provider_ok);

        let research = FundamentalsService::new(DataSourceConfig {
            fmp_api_key: Some("x".into()),
            ..Default::default()
        })
        .expect("service");
        assert!(!research.fetch("0700.HK").await.provider_ok);
    }

    #[tokio::test]
    async fn missing_key_is_honest_missing() {
        let service = FundamentalsService::new(DataSourceConfig::default()).expect("service");
        assert!(!service.fetch("AAPL").await.provider_ok);
    }
}
