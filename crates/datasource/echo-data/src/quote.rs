use crate::{
    AdapterAuthorization, AdapterDescriptor, LicenseTier, Market, QualityReport, check_quote,
    detect_market, normalize_ticker, select_adapter_chain,
};
use chrono::{DateTime, TimeZone, Utc};
use echo_config::DataSourceConfig;
use echo_db::{MarketRepository, MarketSnapshotWrite, Pool};
use rust_decimal::Decimal;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const US: &[Market] = &[Market::Us];
const US_HK: &[Market] = &[Market::Us, Market::Hk];
const FAILURE_THRESHOLD: u8 = 3;
const BREAKER_COOLDOWN: Duration = Duration::from_secs(300);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderStatus {
    Ok,
    Missing,
}

#[derive(Clone, Debug)]
pub struct Quote {
    pub source: String,
    pub ticker: String,
    pub currency: Option<String>,
    pub price: Option<Decimal>,
    pub previous_close: Option<Decimal>,
    pub change: Option<Decimal>,
    pub change_percent: Option<Decimal>,
    pub open: Option<Decimal>,
    pub high: Option<Decimal>,
    pub low: Option<Decimal>,
    pub volume: Option<Decimal>,
    pub market_cap: Option<Decimal>,
    pub pe: Option<Decimal>,
    pub dividend_yield: Option<Decimal>,
    pub week_52_high: Option<Decimal>,
    pub week_52_low: Option<Decimal>,
    pub as_of: DateTime<Utc>,
    pub status: ProviderStatus,
}

impl Quote {
    #[must_use]
    pub fn missing(ticker: &str, source: &str, as_of: DateTime<Utc>) -> Self {
        Self {
            source: source.into(),
            ticker: normalize_ticker(ticker),
            currency: None,
            price: None,
            previous_close: None,
            change: None,
            change_percent: None,
            open: None,
            high: None,
            low: None,
            volume: None,
            market_cap: None,
            pe: None,
            dividend_yield: None,
            week_52_high: None,
            week_52_low: None,
            as_of,
            status: ProviderStatus::Missing,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum QuoteError {
    #[error("不支持的市场: {0}")]
    UnsupportedMarket(String),
    #[error("商用模式没有已授权的行情源: {0}")]
    NoAuthorizedAdapter(String),
    #[error("所有行情源均失败: {0}")]
    ProvidersFailed(String),
    #[error("行情质量门拒绝 {ticker}: {detail}")]
    QualityRejected { ticker: String, detail: String },
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Database(#[from] echo_db::DbError),
}

#[derive(Clone, Debug)]
pub struct RoutedQuote {
    pub quote: Quote,
    pub adapter_id: &'static str,
    pub quality: QualityReport,
}

#[derive(Clone, Copy)]
struct BreakerEntry {
    failures: u8,
    open_until: Option<Instant>,
}

#[derive(Clone)]
pub struct QuoteService {
    client: reqwest::Client,
    pool: Pool,
    config: DataSourceConfig,
    adapters: Vec<AdapterDescriptor>,
    breakers: Arc<Mutex<HashMap<&'static str, BreakerEntry>>>,
}

impl QuoteService {
    pub fn new(pool: Pool, config: DataSourceConfig) -> Result<Self, QuoteError> {
        let client = reqwest::Client::builder()
            .user_agent("EchoResearch/1.0")
            .timeout(Duration::from_secs(10))
            .build()?;
        let mut adapters = Vec::new();
        if config.finnhub_api_key.is_some() {
            adapters.push(AdapterDescriptor {
                id: "finnhub",
                authorization: AdapterAuthorization {
                    license_tier: LicenseTier::UnlicensedFreeTier,
                    commercial_use_allowed: false,
                    latency_p95_ms: None,
                },
                quality_rank: 1,
                markets: US,
            });
        }
        adapters.push(AdapterDescriptor {
            id: "yahoo-chart",
            authorization: AdapterAuthorization {
                license_tier: LicenseTier::UnlicensedFreeTier,
                commercial_use_allowed: false,
                latency_p95_ms: None,
            },
            quality_rank: 3,
            markets: US_HK,
        });
        Ok(Self {
            client,
            pool,
            config,
            adapters,
            breakers: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub async fn fetch_live(&self, raw_ticker: &str) -> Result<RoutedQuote, QuoteError> {
        let ticker = normalize_ticker(raw_ticker);
        let market = detect_market(&ticker);
        if market == Market::Unsupported {
            return Err(QuoteError::UnsupportedMarket(ticker));
        }
        let chain = select_adapter_chain(&self.adapters, market, self.config.commercial_mode);
        if chain.is_empty() {
            return Err(QuoteError::NoAuthorizedAdapter(ticker));
        }
        let mut details = Vec::new();
        for adapter in chain {
            if self.breaker_open(adapter.id) {
                details.push(format!("{} 熔断中", adapter.id));
                continue;
            }
            let result = match adapter.id {
                "finnhub" => self.fetch_finnhub(&ticker).await,
                "yahoo-chart" => self.fetch_yahoo(&ticker).await,
                _ => unreachable!("注册表只含已实现适配器"),
            };
            match result {
                Ok(quote) => {
                    self.record_success(adapter.id);
                    let quality = check_quote(&quote, Utc::now());
                    if !quality.ok {
                        let detail = quality
                            .issues
                            .iter()
                            .map(|issue| issue.message.as_str())
                            .collect::<Vec<_>>()
                            .join("；");
                        return Err(QuoteError::QualityRejected { ticker, detail });
                    }
                    return Ok(RoutedQuote {
                        quote,
                        adapter_id: adapter.id,
                        quality,
                    });
                }
                Err(error) => {
                    self.record_failure(adapter.id);
                    details.push(format!("{}: {error}", adapter.id));
                }
            }
        }
        Err(QuoteError::ProvidersFailed(details.join(" | ")))
    }

    pub async fn refresh(&self, ticker: &str) -> Result<RoutedQuote, QuoteError> {
        let routed = self.fetch_live(ticker).await?;
        if routed.quote.status == ProviderStatus::Ok {
            MarketRepository::new(&self.pool)
                .insert_snapshot(&MarketSnapshotWrite::from(&routed.quote))
                .await?;
        }
        Ok(routed)
    }

    fn breaker_open(&self, id: &'static str) -> bool {
        let mut states = self.breakers.lock().expect("breaker mutex poisoned");
        let Some(entry) = states.get(&id).copied() else {
            return false;
        };
        if entry.open_until.is_some_and(|until| Instant::now() < until) {
            return true;
        }
        if entry.open_until.is_some() {
            states.remove(id);
        }
        false
    }

    fn record_success(&self, id: &'static str) {
        self.breakers
            .lock()
            .expect("breaker mutex poisoned")
            .remove(id);
    }

    fn record_failure(&self, id: &'static str) {
        let mut states = self.breakers.lock().expect("breaker mutex poisoned");
        let entry = states.entry(id).or_insert(BreakerEntry {
            failures: 0,
            open_until: None,
        });
        entry.failures = entry.failures.saturating_add(1);
        if entry.failures >= FAILURE_THRESHOLD {
            entry.open_until = Some(Instant::now() + BREAKER_COOLDOWN);
        }
    }

    async fn fetch_finnhub(&self, ticker: &str) -> Result<Quote, QuoteError> {
        let key = self
            .config
            .finnhub_api_key
            .as_deref()
            .expect("adapter registered with key");
        let body: Value = self
            .client
            .get("https://finnhub.io/api/v1/quote")
            .query(&[("symbol", ticker), ("token", key)])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        if let Some(message) = body.get("error").and_then(Value::as_str) {
            return Err(QuoteError::ProvidersFailed(message.into()));
        }
        let price = decimal_at(&body, "c").filter(|value| *value > Decimal::ZERO);
        let previous_close = decimal_at(&body, "pc").filter(|value| *value > Decimal::ZERO);
        let change =
            decimal_at(&body, "d").or_else(|| price.zip(previous_close).map(|(a, b)| a - b));
        let change_percent = decimal_at(&body, "dp").or_else(|| {
            change.zip(previous_close).and_then(|(delta, previous)| {
                (!previous.is_zero()).then(|| delta * Decimal::from(100) / previous)
            })
        });
        let as_of = body
            .get("t")
            .and_then(Value::as_i64)
            .filter(|value| *value > 0)
            .and_then(|seconds| Utc.timestamp_opt(seconds, 0).single())
            .unwrap_or_else(Utc::now);
        Ok(Quote {
            source: "finnhub".into(),
            ticker: ticker.into(),
            currency: Some("USD".into()),
            price,
            previous_close,
            change,
            change_percent,
            open: decimal_at(&body, "o"),
            high: decimal_at(&body, "h"),
            low: decimal_at(&body, "l"),
            volume: None,
            market_cap: None,
            pe: None,
            dividend_yield: None,
            week_52_high: None,
            week_52_low: None,
            as_of,
            status: if price.is_some() {
                ProviderStatus::Ok
            } else {
                ProviderStatus::Missing
            },
        })
    }

    async fn fetch_yahoo(&self, ticker: &str) -> Result<Quote, QuoteError> {
        let mut last_error = None;
        for host in ["query2.finance.yahoo.com", "query1.finance.yahoo.com"] {
            let url = format!("https://{host}/v8/finance/chart/{ticker}");
            match self
                .client
                .get(url)
                .query(&[("range", "5d"), ("interval", "1d")])
                .send()
                .await
            {
                Ok(response) if response.status().is_success() => {
                    let body: Value = response.json().await?;
                    return yahoo_quote(ticker, &body);
                }
                Ok(response) => last_error = Some(format!("HTTP {}", response.status())),
                Err(error) => last_error = Some(error.to_string()),
            }
        }
        Err(QuoteError::ProvidersFailed(
            last_error.unwrap_or_else(|| "Yahoo 无响应".into()),
        ))
    }
}

impl From<&Quote> for MarketSnapshotWrite {
    fn from(quote: &Quote) -> Self {
        Self {
            ticker: quote.ticker.clone(),
            price: quote.price,
            previous_close: quote.previous_close,
            change: quote.change,
            change_percent: quote.change_percent,
            open: quote.open,
            high: quote.high,
            low: quote.low,
            volume: quote.volume,
            market_cap: quote.market_cap,
            pe: quote.pe,
            dividend_yield: quote.dividend_yield,
            week_52_high: quote.week_52_high,
            week_52_low: quote.week_52_low,
            source: quote.source.clone(),
            valid_time: quote.as_of,
        }
    }
}

fn decimal(value: &Value) -> Option<Decimal> {
    match value {
        Value::Number(number) => number.to_string().parse().ok(),
        Value::String(value) => value.parse().ok(),
        _ => None,
    }
}

fn decimal_at(body: &Value, key: &str) -> Option<Decimal> {
    body.get(key).and_then(decimal)
}

fn nested<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter()
        .try_fold(value, |current, key| current.get(*key))
}

fn last_decimal(value: Option<&Value>) -> Option<Decimal> {
    value?.as_array()?.iter().rev().find_map(decimal)
}

fn yahoo_quote(ticker: &str, body: &Value) -> Result<Quote, QuoteError> {
    if let Some(error) = nested(body, &["chart", "error"]).filter(|value| !value.is_null()) {
        return Err(QuoteError::ProvidersFailed(error.to_string()));
    }
    let result = nested(body, &["chart", "result"])
        .and_then(Value::as_array)
        .and_then(|results| results.first())
        .ok_or_else(|| QuoteError::ProvidersFailed(format!("Yahoo 未返回 {ticker}")))?;
    let meta = result.get("meta").unwrap_or(&Value::Null);
    let quote_values = nested(result, &["indicators", "quote"])
        .and_then(Value::as_array)
        .and_then(|values| values.first())
        .unwrap_or(&Value::Null);
    let price = decimal_at(meta, "regularMarketPrice");
    let previous_close = decimal_at(meta, "regularMarketPreviousClose")
        .or_else(|| decimal_at(meta, "chartPreviousClose"))
        .or_else(|| decimal_at(meta, "previousClose"));
    let change = price
        .zip(previous_close)
        .map(|(current, previous)| current - previous);
    let change_percent = change.zip(previous_close).and_then(|(delta, previous)| {
        (!previous.is_zero()).then(|| delta * Decimal::from(100) / previous)
    });
    let as_of = meta
        .get("regularMarketTime")
        .and_then(Value::as_i64)
        .and_then(|seconds| Utc.timestamp_opt(seconds, 0).single())
        .unwrap_or_else(Utc::now);
    Ok(Quote {
        source: "yahoo".into(),
        ticker: ticker.into(),
        currency: meta
            .get("currency")
            .and_then(Value::as_str)
            .map(str::to_string),
        price,
        previous_close,
        change,
        change_percent,
        open: last_decimal(quote_values.get("open")),
        high: decimal_at(meta, "regularMarketDayHigh")
            .or_else(|| last_decimal(quote_values.get("high"))),
        low: decimal_at(meta, "regularMarketDayLow")
            .or_else(|| last_decimal(quote_values.get("low"))),
        volume: decimal_at(meta, "regularMarketVolume")
            .or_else(|| last_decimal(quote_values.get("volume"))),
        market_cap: None,
        pe: None,
        dividend_yield: None,
        week_52_high: decimal_at(meta, "fiftyTwoWeekHigh"),
        week_52_low: decimal_at(meta, "fiftyTwoWeekLow"),
        as_of,
        status: if price.is_some() {
            ProviderStatus::Ok
        } else {
            ProviderStatus::Missing
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn yahoo_payload_maps_without_binary_float() {
        let body = serde_json::json!({"chart":{"result":[{"meta":{
            "currency":"HKD","regularMarketPrice":601.25,"regularMarketPreviousClose":600,
            "regularMarketTime":1784592000},"indicators":{"quote":[{"open":[null,598.5],
            "high":[602],"low":[595],"volume":[1200]}]}}],"error":null}});
        let quote = yahoo_quote("0700.HK", &body).expect("quote");
        assert_eq!(quote.price, Some(dec!(601.25)));
        assert_eq!(quote.change, Some(dec!(1.25)));
        assert_eq!(quote.currency.as_deref(), Some("HKD"));
    }
}
