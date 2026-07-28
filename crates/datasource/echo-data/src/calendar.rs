//! Finnhub 财报日历——唯一取数入口的持久化侧读写（对齐 `earnings_calendar` 表）。
//!
//! 免费档 `commercial_use_allowed = false`，与 `quote.rs` 的 finnhub 适配器同一授权口径；
//! 商用模式下直接拒绝，不悄悄降级成"能用但未授权"。

use crate::{Market, detect_market, normalize_ticker};
use chrono::{Duration as ChronoDuration, Utc};
use echo_config::DataSourceConfig;
use echo_db::{CalendarRepository, CalendarRow, CalendarUpsert, Pool};
use rust_decimal::Decimal;
use serde_json::Value;
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum CalendarError {
    #[error("FINNHUB_API_KEY 未配置")]
    MissingApiKey,
    #[error("商用模式不允许未授权的 Finnhub 免费源")]
    CommercialBlocked,
    #[error("Finnhub 财报日历不支持该市场：{0}")]
    UnsupportedMarket(String),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Db(#[from] echo_db::DbError),
}

/// 读库缓存超过此窗口视为过期，需重新向 Finnhub 取数。
const STALE_AFTER: ChronoDuration = ChronoDuration::hours(24);

#[derive(Clone)]
pub struct CalendarService {
    client: reqwest::Client,
    pool: Pool,
    config: DataSourceConfig,
}

impl CalendarService {
    pub fn new(pool: Pool, config: DataSourceConfig) -> Result<Self, CalendarError> {
        let client = reqwest::Client::builder()
            .user_agent("EchoResearch/1.0")
            .timeout(Duration::from_secs(8))
            .build()?;
        Ok(Self {
            client,
            pool,
            config,
        })
    }

    /// 读库优先；缺行或超过 [`STALE_AFTER`] 才回源 Finnhub 并回写。失败即诚实返回 `None`，
    /// 绝不用陈旧值冒充最新。
    pub async fn load(&self, raw_ticker: &str) -> Option<CalendarRow> {
        let ticker = normalize_ticker(raw_ticker);
        let repo = CalendarRepository::new(&self.pool);
        if let Ok(Some(row)) = repo.latest(&ticker).await {
            if Utc::now() - row.knowledge_time < STALE_AFTER {
                return Some(row);
            }
        }
        match self.refresh(&ticker).await {
            Ok(()) => repo.latest(&ticker).await.ok().flatten(),
            Err(error) => {
                tracing::warn!(ticker = %ticker, error = %error, "Finnhub 财报日历未核到");
                repo.latest(&ticker).await.ok().flatten()
            }
        }
    }

    async fn refresh(&self, ticker: &str) -> Result<(), CalendarError> {
        if self.config.commercial_mode {
            return Err(CalendarError::CommercialBlocked);
        }
        let Some(api_key) = self.config.finnhub_api_key.as_deref() else {
            return Err(CalendarError::MissingApiKey);
        };
        if detect_market(ticker) == Market::Unsupported {
            return Err(CalendarError::UnsupportedMarket(ticker.into()));
        }
        let from = Utc::now().date_naive();
        let to = from + ChronoDuration::days(180);
        let body: Value = self
            .client
            .get("https://finnhub.io/api/v1/calendar/earnings")
            .query(&[
                ("symbol", ticker),
                ("from", &from.to_string()),
                ("to", &to.to_string()),
                ("token", api_key),
            ])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let Some(next) = body
            .get("earningsCalendar")
            .and_then(Value::as_array)
            .and_then(|rows| rows.first())
        else {
            return Ok(());
        };
        CalendarRepository::new(&self.pool)
            .upsert(&CalendarUpsert {
                ticker: ticker.into(),
                next_date: next.get("date").and_then(Value::as_str).map(str::to_string),
                quarter: next
                    .get("quarter")
                    .and_then(Value::as_i64)
                    .map(|v| v as i32),
                year: next.get("year").and_then(Value::as_i64).map(|v| v as i32),
                eps_estimate: decimal_at(next, "epsEstimate"),
                revenue_estimate: decimal_at(next, "revenueEstimate"),
                source: "finnhub".into(),
            })
            .await?;
        Ok(())
    }
}

fn decimal_at(body: &Value, key: &str) -> Option<Decimal> {
    match body.get(key)? {
        Value::Number(number) => number.to_string().parse().ok(),
        Value::String(text) => text.parse().ok(),
        _ => None,
    }
}
