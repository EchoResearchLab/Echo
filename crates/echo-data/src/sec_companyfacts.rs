//! SEC Company Facts 官方财务补救源（美股）。
//!
//! 主财务供应商缺 TTM EPS / 三表时才调用。流量遵守 SEC 的可联系 User-Agent 要求；没有
//! `SEC_USER_AGENT` 就不启用。TTM 流量指标按“最近年报 + 当前 YTD - 上年同期 YTD”计算，
//! 无法严格拼出 TTM 时退回最近 FY 并明确标成 annual（仍是已年化口径，不拿单季冒充）。

use crate::{Market, detect_market, normalize_ticker};
use chrono::NaiveDate;
use echo_config::DataSourceConfig;
use rust_decimal::Decimal;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

const TICKERS_URL: &str = "https://www.sec.gov/files/company_tickers.json";
const COMPANY_FACTS_BASE: &str = "https://data.sec.gov/api/xbrl/companyfacts";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecPeriodBasis {
    Ttm,
    Annual,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SecFundamentals {
    pub company_name: Option<String>,
    pub currency: Option<String>,
    pub revenue: Option<Decimal>,
    pub gross_profit: Option<Decimal>,
    pub operating_income: Option<Decimal>,
    pub net_income: Option<Decimal>,
    pub operating_cash_flow: Option<Decimal>,
    pub cash_and_equivalents: Option<Decimal>,
    pub eps: Option<Decimal>,
    pub shares_outstanding: Option<Decimal>,
    pub period_end: Option<String>,
    pub basis: Option<SecPeriodBasis>,
}

impl SecFundamentals {
    #[must_use]
    pub fn provider_ok(&self) -> bool {
        self.revenue.is_some() || self.net_income.is_some() || self.eps.is_some()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SecCompanyFactsError {
    #[error("SEC_USER_AGENT 未配置")]
    MissingUserAgent,
    #[error("SEC Company Facts 仅支持美股：{0}")]
    UnsupportedMarket(String),
    #[error("SEC ticker 映射没有 {0}")]
    CikMissing(String),
    #[error("SEC 返回格式无效：{0}")]
    InvalidResponse(String),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
}

#[derive(Clone)]
pub struct SecCompanyFactsService {
    client: reqwest::Client,
    user_agent_configured: bool,
    cik_cache: Arc<RwLock<Option<HashMap<String, u64>>>>,
}

impl SecCompanyFactsService {
    pub fn new(config: DataSourceConfig) -> Result<Self, SecCompanyFactsError> {
        let user_agent = config
            .sec_user_agent
            .as_deref()
            .ok_or(SecCompanyFactsError::MissingUserAgent)?;
        let client = reqwest::Client::builder()
            .user_agent(user_agent)
            .timeout(Duration::from_secs(12))
            .build()?;
        Ok(Self {
            client,
            user_agent_configured: true,
            cik_cache: Arc::new(RwLock::new(None)),
        })
    }

    #[must_use]
    pub const fn is_configured(&self) -> bool {
        self.user_agent_configured
    }

    pub async fn fetch(&self, raw_ticker: &str) -> Result<SecFundamentals, SecCompanyFactsError> {
        let ticker = normalize_ticker(raw_ticker);
        if detect_market(&ticker) != Market::Us {
            return Err(SecCompanyFactsError::UnsupportedMarket(ticker));
        }
        let cik = self.cik_for(&ticker).await?;
        let url = format!("{COMPANY_FACTS_BASE}/CIK{cik:010}.json");
        let body: Value = self
            .client
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(map_company_facts(&body))
    }

    async fn cik_for(&self, ticker: &str) -> Result<u64, SecCompanyFactsError> {
        if let Some(cik) = self
            .cik_cache
            .read()
            .expect("SEC CIK cache poisoned")
            .as_ref()
            .and_then(|cache| cache.get(ticker).copied())
        {
            return Ok(cik);
        }
        let body: Value = self
            .client
            .get(TICKERS_URL)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let cache = parse_cik_map(&body);
        let cik = cache
            .get(ticker)
            .copied()
            .ok_or_else(|| SecCompanyFactsError::CikMissing(ticker.into()))?;
        *self.cik_cache.write().expect("SEC CIK cache poisoned") = Some(cache);
        Ok(cik)
    }
}

#[derive(Clone, Debug)]
struct FactEntry {
    start: Option<NaiveDate>,
    end: NaiveDate,
    filed: NaiveDate,
    value: Decimal,
    form: String,
    fp: Option<String>,
}

fn parse_cik_map(body: &Value) -> HashMap<String, u64> {
    body.as_object()
        .into_iter()
        .flat_map(|rows| rows.values())
        .filter_map(|row| {
            let ticker = row.get("ticker")?.as_str()?.trim().to_ascii_uppercase();
            let cik = row.get("cik_str")?.as_u64()?;
            Some((ticker, cik))
        })
        .collect()
}

fn map_company_facts(body: &Value) -> SecFundamentals {
    let revenue_entries = fact_entries(
        body,
        &[
            "RevenueFromContractWithCustomerExcludingAssessedTax",
            "Revenues",
            "SalesRevenueNet",
        ],
        "USD",
    );
    let gross_entries = fact_entries(body, &["GrossProfit"], "USD");
    let operating_entries = fact_entries(body, &["OperatingIncomeLoss"], "USD");
    let income_entries = fact_entries(body, &["NetIncomeLoss", "ProfitLoss"], "USD");
    let cash_flow_entries =
        fact_entries(body, &["NetCashProvidedByUsedInOperatingActivities"], "USD");
    let eps_entries = fact_entries(
        body,
        &["EarningsPerShareDiluted", "EarningsPerShareBasicAndDiluted"],
        "USD/shares",
    );
    let cash_entries = fact_entries(
        body,
        &[
            "CashAndCashEquivalentsAtCarryingValue",
            "CashCashEquivalentsRestrictedCashAndRestrictedCashEquivalents",
        ],
        "USD",
    );
    let shares_entries = fact_entries_namespace(
        body,
        "dei",
        &["EntityCommonStockSharesOutstanding"],
        "shares",
    );

    let revenue = ttm_or_annual(&revenue_entries);
    let gross = ttm_or_annual(&gross_entries);
    let operating = ttm_or_annual(&operating_entries);
    let income = ttm_or_annual(&income_entries);
    let cash_flow = ttm_or_annual(&cash_flow_entries);
    let eps = ttm_or_annual(&eps_entries);
    let period_end = [revenue.as_ref(), income.as_ref(), eps.as_ref()]
        .into_iter()
        .flatten()
        .map(|value| value.end)
        .max();
    let basis = [revenue.as_ref(), income.as_ref(), eps.as_ref()]
        .into_iter()
        .flatten()
        .map(|value| value.basis)
        .max_by_key(|basis| matches!(basis, SecPeriodBasis::Ttm));

    SecFundamentals {
        company_name: body
            .get("entityName")
            .and_then(Value::as_str)
            .map(str::to_string),
        currency: Some("USD".into()),
        revenue: revenue.map(|value| value.value),
        gross_profit: gross.map(|value| value.value),
        operating_income: operating.map(|value| value.value),
        net_income: income.map(|value| value.value),
        operating_cash_flow: cash_flow.map(|value| value.value),
        cash_and_equivalents: latest_point(&cash_entries).map(|entry| entry.value),
        eps: eps.map(|value| value.value),
        shares_outstanding: latest_point(&shares_entries).map(|entry| entry.value),
        period_end: period_end.map(|date| date.to_string()),
        basis,
    }
}

#[derive(Clone, Copy)]
struct FlowValue {
    value: Decimal,
    end: NaiveDate,
    basis: SecPeriodBasis,
}

fn ttm_or_annual(entries: &[FactEntry]) -> Option<FlowValue> {
    let annual = entries
        .iter()
        .filter(|entry| {
            entry.form == "10-K"
                && entry.fp.as_deref() == Some("FY")
                && duration_days(entry).is_some_and(|days| (250..=450).contains(&days))
        })
        .max_by_key(|entry| (entry.end, entry.filed))?;
    let current = entries
        .iter()
        .filter(|entry| {
            entry.form == "10-Q"
                && entry.end > annual.end
                && matches!(entry.fp.as_deref(), Some("Q1" | "Q2" | "Q3"))
                && duration_days(entry).is_some_and(|days| (45..=320).contains(&days))
        })
        // 同一 end 同时有季度值和 YTD 值时选 start 更早（累计期更长）的那一行。
        .max_by_key(|entry| {
            (
                entry.end,
                entry
                    .start
                    .map(|start| (entry.end - start).num_days())
                    .unwrap_or_default(),
                entry.filed,
            )
        });
    if let Some(current) = current {
        let current_days = duration_days(current)?;
        let prior = entries
            .iter()
            .filter(|entry| {
                entry.form == "10-Q"
                    && entry.fp == current.fp
                    && entry.end < current.end
                    && {
                        let age = (current.end - entry.end).num_days();
                        (300..=430).contains(&age)
                    }
                    && duration_days(entry).is_some_and(|days| (days - current_days).abs() <= 35)
            })
            .max_by_key(|entry| (entry.end, entry.filed));
        if let Some(prior) = prior {
            return Some(FlowValue {
                value: annual.value + current.value - prior.value,
                end: current.end,
                basis: SecPeriodBasis::Ttm,
            });
        }
    }
    Some(FlowValue {
        value: annual.value,
        end: annual.end,
        basis: SecPeriodBasis::Annual,
    })
}

fn duration_days(entry: &FactEntry) -> Option<i64> {
    entry.start.map(|start| (entry.end - start).num_days())
}

fn latest_point(entries: &[FactEntry]) -> Option<&FactEntry> {
    entries.iter().max_by_key(|entry| (entry.end, entry.filed))
}

fn fact_entries(body: &Value, tags: &[&str], unit: &str) -> Vec<FactEntry> {
    fact_entries_namespace(body, "us-gaap", tags, unit)
}

fn fact_entries_namespace(
    body: &Value,
    namespace: &str,
    tags: &[&str],
    unit: &str,
) -> Vec<FactEntry> {
    for tag in tags {
        let Some(values) = body
            .get("facts")
            .and_then(|facts| facts.get(namespace))
            .and_then(|facts| facts.get(tag))
            .and_then(|fact| fact.get("units"))
            .and_then(|units| units.get(unit))
            .and_then(Value::as_array)
        else {
            continue;
        };
        let entries = values.iter().filter_map(parse_entry).collect::<Vec<_>>();
        if !entries.is_empty() {
            return entries;
        }
    }
    Vec::new()
}

fn parse_entry(value: &Value) -> Option<FactEntry> {
    let form = value.get("form")?.as_str()?.to_string();
    if !matches!(form.as_str(), "10-Q" | "10-K") {
        return None;
    }
    Some(FactEntry {
        start: value
            .get("start")
            .and_then(Value::as_str)
            .and_then(parse_date),
        end: parse_date(value.get("end")?.as_str()?)?,
        filed: parse_date(value.get("filed")?.as_str()?)?,
        value: decimal(value.get("val")?)?,
        form,
        fp: value.get("fp").and_then(Value::as_str).map(str::to_string),
    })
}

fn parse_date(value: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()
}

fn decimal(value: &Value) -> Option<Decimal> {
    match value {
        Value::Number(number) => number.to_string().parse().ok(),
        Value::String(text) => text.parse().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;
    use serde_json::json;

    fn entry(start: &str, end: &str, filed: &str, value: i64, form: &str, fp: &str) -> Value {
        json!({
            "start": start, "end": end, "filed": filed, "val": value,
            "form": form, "fp": fp
        })
    }

    #[test]
    fn ttm_uses_annual_plus_current_ytd_minus_prior_ytd() {
        let entries = vec![
            parse_entry(&entry(
                "2024-01-01",
                "2024-12-31",
                "2025-02-10",
                100,
                "10-K",
                "FY",
            ))
            .unwrap(),
            parse_entry(&entry(
                "2025-01-01",
                "2025-06-30",
                "2025-08-01",
                70,
                "10-Q",
                "Q2",
            ))
            .unwrap(),
            parse_entry(&entry(
                "2024-01-01",
                "2024-06-30",
                "2024-08-01",
                50,
                "10-Q",
                "Q2",
            ))
            .unwrap(),
        ];
        let value = ttm_or_annual(&entries).expect("ttm");
        assert_eq!(value.value, dec!(120));
        assert_eq!(value.basis, SecPeriodBasis::Ttm);
    }

    #[test]
    fn mapping_uses_official_eps_and_shares_without_float() {
        let body = json!({
            "entityName": "Example Inc.",
            "facts": {
                "us-gaap": {
                    "EarningsPerShareDiluted": {"units": {"USD/shares": [
                        {"start":"2024-01-01","end":"2024-12-31","filed":"2025-02-10",
                         "val":"8.25","form":"10-K","fp":"FY"}
                    ]}}
                },
                "dei": {
                    "EntityCommonStockSharesOutstanding": {"units": {"shares": [
                        {"end":"2025-06-30","filed":"2025-08-01",
                         "val":1000000,"form":"10-Q","fp":"Q2"}
                    ]}}
                }
            }
        });
        let mapped = map_company_facts(&body);
        assert_eq!(mapped.eps, Some(dec!(8.25)));
        assert_eq!(mapped.shares_outstanding, Some(dec!(1000000)));
        assert_eq!(mapped.basis, Some(SecPeriodBasis::Annual));
    }
}
