//! 港股业绩公告金额归一化边界。
//!
//! HKEX 公告常以元、千元、百万元或十亿元展示金额。写库前必须把来源单位显式转换为绝对值，
//! 同时保留倍率和解析器版本；读侧只允许 `amounts_normalized=true` 的绝对值进入估值。
//! 历史行没有这个证明，继续只能用于同一行内的单位无关比率。

use chrono::{DateTime, Utc};
use echo_db::{HkFinancialsRepository, HkFinancialsUpsert, Pool};
use rust_decimal::Decimal;
use serde::Deserialize;
use url::Url;

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawHkFinancials {
    pub ticker: String,
    pub period_label: Option<String>,
    pub period_end: Option<String>,
    pub period_type: Option<String>,
    pub currency: String,
    pub source_unit: String,
    pub revenue: Option<Decimal>,
    pub revenue_prior: Option<Decimal>,
    pub gross_profit: Option<Decimal>,
    pub gross_profit_prior: Option<Decimal>,
    pub operating_income: Option<Decimal>,
    pub operating_income_prior: Option<Decimal>,
    pub net_income: Option<Decimal>,
    pub net_income_prior: Option<Decimal>,
    pub net_income_attributable: Option<Decimal>,
    pub eps: Option<Decimal>,
    pub operating_cash_flow: Option<Decimal>,
    pub cash_and_equivalents: Option<Decimal>,
    pub net_cash: Option<Decimal>,
    pub free_cash_flow: Option<Decimal>,
    pub source_title: String,
    pub source_url: String,
    pub published_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NormalizedHkFinancials {
    pub ticker: String,
    pub period_label: Option<String>,
    pub period_end: Option<String>,
    pub period_type: Option<String>,
    pub currency: String,
    pub source_unit: String,
    pub source_unit_scale: Decimal,
    pub revenue: Option<Decimal>,
    pub revenue_prior: Option<Decimal>,
    pub gross_profit: Option<Decimal>,
    pub gross_profit_prior: Option<Decimal>,
    pub operating_income: Option<Decimal>,
    pub operating_income_prior: Option<Decimal>,
    pub net_income: Option<Decimal>,
    pub net_income_prior: Option<Decimal>,
    pub net_income_attributable: Option<Decimal>,
    pub eps: Option<Decimal>,
    pub operating_cash_flow: Option<Decimal>,
    pub cash_and_equivalents: Option<Decimal>,
    pub net_cash: Option<Decimal>,
    pub free_cash_flow: Option<Decimal>,
    pub source_title: String,
    pub source_url: String,
    pub published_at: Option<DateTime<Utc>>,
    pub parser_version: &'static str,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum HkFinancialsError {
    #[error("只接受港股代码（例如 0700.HK）")]
    InvalidTicker,
    #[error("只接受 HKEX 披露易 HTTPS 来源")]
    InvalidSourceUrl,
    #[error("报告币种只允许 HKD/CNY/USD")]
    InvalidCurrency,
    #[error("无法识别公告金额单位")]
    InvalidUnit,
    #[error("至少需要收入或净利润")]
    MissingCoreAmount,
    #[error("收入必须为正数")]
    InvalidRevenue,
    #[error("金额归一化后超出可信范围")]
    AmountOutOfRange,
}

/// 受控 ingest 当前版本。变更单位识别或数值选择规则时必须升级，便于库内追溯。
pub const HK_FINANCIALS_PARSER_VERSION: &str = "hkex-structured-v1";

#[must_use]
pub fn source_unit_scale(unit: &str) -> Option<Decimal> {
    let compact = unit
        .trim()
        .replace([' ', '（', '）', '(', ')'], "")
        .to_ascii_lowercase();
    match compact.as_str() {
        "元" | "港元" | "人民币元" | "人民幣元" | "hkd" | "cny" | "rmb" | "usd" | "dollar"
        | "dollars" => Some(Decimal::ONE),
        "千" | "千元" | "hkd'000" | "rmb'000" | "usd'000" | "thousand" | "thousands" => {
            Some(Decimal::from(1_000))
        }
        "百万" | "百萬元" | "百万元" | "million" | "millions" => {
            Some(Decimal::from(1_000_000))
        }
        "十亿" | "十億元" | "十亿元" | "billion" | "billions" => {
            Some(Decimal::from(1_000_000_000))
        }
        _ => None,
    }
}

pub fn normalize_hk_financials(
    raw: RawHkFinancials,
) -> Result<NormalizedHkFinancials, HkFinancialsError> {
    let ticker_input = raw.ticker.trim().to_ascii_uppercase();
    if !valid_hk_ticker(&ticker_input) {
        return Err(HkFinancialsError::InvalidTicker);
    }
    let code = ticker_input
        .strip_suffix(".HK")
        .expect("valid_hk_ticker guarantees suffix");
    let ticker = format!("{code:0>4}.HK");
    if !is_hkex_source(&raw.source_url) {
        return Err(HkFinancialsError::InvalidSourceUrl);
    }
    let currency = raw.currency.trim().to_ascii_uppercase();
    if !matches!(currency.as_str(), "HKD" | "CNY" | "USD") {
        return Err(HkFinancialsError::InvalidCurrency);
    }
    let scale = source_unit_scale(&raw.source_unit).ok_or(HkFinancialsError::InvalidUnit)?;
    if raw.revenue.is_none() && raw.net_income.is_none() {
        return Err(HkFinancialsError::MissingCoreAmount);
    }
    if raw.revenue.is_some_and(|value| value <= Decimal::ZERO) {
        return Err(HkFinancialsError::InvalidRevenue);
    }

    let scale_amount = |value: Option<Decimal>| -> Result<Option<Decimal>, HkFinancialsError> {
        let Some(value) = value else {
            return Ok(None);
        };
        let absolute = value
            .checked_mul(scale)
            .ok_or(HkFinancialsError::AmountOutOfRange)?;
        // 单家公司单项金额上限 10^15；超过通常意味着单位被重复放大或匹配错了表格列。
        if absolute.abs() > Decimal::from_i128_with_scale(1_000_000_000_000_000, 0) {
            return Err(HkFinancialsError::AmountOutOfRange);
        }
        Ok(Some(absolute))
    };

    Ok(NormalizedHkFinancials {
        ticker,
        period_label: clean_optional(raw.period_label),
        period_end: clean_optional(raw.period_end),
        period_type: clean_optional(raw.period_type),
        currency,
        source_unit: raw.source_unit.trim().to_string(),
        source_unit_scale: scale,
        revenue: scale_amount(raw.revenue)?,
        revenue_prior: scale_amount(raw.revenue_prior)?,
        gross_profit: scale_amount(raw.gross_profit)?,
        gross_profit_prior: scale_amount(raw.gross_profit_prior)?,
        operating_income: scale_amount(raw.operating_income)?,
        operating_income_prior: scale_amount(raw.operating_income_prior)?,
        net_income: scale_amount(raw.net_income)?,
        net_income_prior: scale_amount(raw.net_income_prior)?,
        net_income_attributable: scale_amount(raw.net_income_attributable)?,
        eps: raw.eps,
        operating_cash_flow: scale_amount(raw.operating_cash_flow)?,
        cash_and_equivalents: scale_amount(raw.cash_and_equivalents)?,
        net_cash: scale_amount(raw.net_cash)?,
        free_cash_flow: scale_amount(raw.free_cash_flow)?,
        source_title: raw.source_title.trim().to_string(),
        source_url: raw.source_url.trim().to_string(),
        published_at: raw.published_at,
        parser_version: HK_FINANCIALS_PARSER_VERSION,
    })
}

/// 单一受控写入口：先做来源/单位/数量级校验，再把归一化证明与绝对金额原子写库。
pub async fn ingest_hk_financials(
    pool: &Pool,
    raw: RawHkFinancials,
) -> Result<NormalizedHkFinancials, HkFinancialsIngestError> {
    let normalized = normalize_hk_financials(raw)?;
    HkFinancialsRepository::new(pool)
        .upsert_normalized(&HkFinancialsUpsert {
            ticker: normalized.ticker.clone(),
            period_label: normalized.period_label.clone(),
            period_end: normalized.period_end.clone(),
            period_type: normalized.period_type.clone(),
            currency: normalized.currency.clone(),
            unit_label: normalized.source_unit.clone(),
            source_unit_scale: normalized.source_unit_scale,
            revenue: normalized.revenue,
            revenue_prior: normalized.revenue_prior,
            gross_profit: normalized.gross_profit,
            gross_profit_prior: normalized.gross_profit_prior,
            operating_income: normalized.operating_income,
            operating_income_prior: normalized.operating_income_prior,
            net_income: normalized.net_income,
            net_income_prior: normalized.net_income_prior,
            net_income_attributable: normalized.net_income_attributable,
            eps: normalized.eps,
            operating_cash_flow: normalized.operating_cash_flow,
            cash_and_equivalents: normalized.cash_and_equivalents,
            net_cash: normalized.net_cash,
            free_cash_flow: normalized.free_cash_flow,
            source_title: normalized.source_title.clone(),
            source_url: normalized.source_url.clone(),
            published_at: normalized.published_at,
            parser_version: normalized.parser_version.to_string(),
        })
        .await?;
    Ok(normalized)
}

#[derive(Debug, thiserror::Error)]
pub enum HkFinancialsIngestError {
    #[error(transparent)]
    Invalid(#[from] HkFinancialsError),
    #[error(transparent)]
    Database(#[from] echo_db::DbError),
}

fn valid_hk_ticker(ticker: &str) -> bool {
    let Some(code) = ticker.strip_suffix(".HK") else {
        return false;
    };
    (1..=5).contains(&code.len()) && code.bytes().all(|byte| byte.is_ascii_digit())
}

fn is_hkex_source(source_url: &str) -> bool {
    let Ok(url) = Url::parse(source_url.trim()) else {
        return false;
    };
    if url.scheme() != "https" || !url.path().to_ascii_lowercase().ends_with(".pdf") {
        return false;
    }
    url.host_str().is_some_and(|host| {
        let host = host.to_ascii_lowercase();
        host == "hkexnews.hk"
            || host.ends_with(".hkexnews.hk")
            || host == "hkex.com.hk"
            || host.ends_with(".hkex.com.hk")
    })
}

fn clean_optional(value: Option<String>) -> Option<String> {
    value
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn raw(unit: &str) -> RawHkFinancials {
        RawHkFinancials {
            ticker: "700.hk".into(),
            period_label: Some(" FY2025 ".into()),
            period_end: Some("2025-12-31".into()),
            period_type: Some("FY".into()),
            currency: "cny".into(),
            source_unit: unit.into(),
            revenue: Some(dec!(751766)),
            revenue_prior: Some(dec!(660257)),
            gross_profit: Some(dec!(427366)),
            gross_profit_prior: None,
            operating_income: None,
            operating_income_prior: None,
            net_income: Some(dec!(228011)),
            net_income_prior: None,
            net_income_attributable: None,
            eps: Some(dec!(24.56)),
            operating_cash_flow: None,
            cash_and_equivalents: None,
            net_cash: None,
            free_cash_flow: None,
            source_title: "年度业绩".into(),
            source_url: "https://www1.hkexnews.hk/listedco/listconews/sehk/2026/ann.pdf".into(),
            published_at: None,
        }
    }

    #[test]
    fn million_source_is_scaled_once_and_eps_is_not_scaled() {
        let normalized = normalize_hk_financials(raw("百萬元")).expect("normalized");
        assert_eq!(normalized.ticker, "0700.HK");
        assert_eq!(normalized.source_unit_scale, dec!(1_000_000));
        assert_eq!(normalized.revenue, Some(dec!(751_766_000_000)));
        assert_eq!(normalized.eps, Some(dec!(24.56)));
        assert_eq!(normalized.parser_version, HK_FINANCIALS_PARSER_VERSION);
    }

    #[test]
    fn unit_aliases_are_explicit_not_guessed() {
        assert_eq!(source_unit_scale("HKD'000"), Some(dec!(1_000)));
        assert_eq!(source_unit_scale("十億元"), Some(dec!(1_000_000_000)));
        assert_eq!(source_unit_scale("约百万元"), None);
    }

    #[test]
    fn rejects_non_hkex_source_and_missing_unit() {
        let mut wrong_source = raw("百萬元");
        wrong_source.source_url = "https://example.com/fake.pdf".into();
        assert_eq!(
            normalize_hk_financials(wrong_source).unwrap_err(),
            HkFinancialsError::InvalidSourceUrl
        );
        assert_eq!(
            normalize_hk_financials(raw("")).unwrap_err(),
            HkFinancialsError::InvalidUnit
        );
    }

    #[test]
    fn negative_revenue_is_rejected_before_storage() {
        let mut input = raw("百萬元");
        input.revenue = Some(dec!(-1));
        assert_eq!(
            normalize_hk_financials(input).unwrap_err(),
            HkFinancialsError::InvalidRevenue
        );
    }
}
