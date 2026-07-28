//! 公司公告/披露（`company_filings`）——Finnhub SEC filings index，唯一取数入口的
//! 持久化侧读写。美股专属（EDGAR 本身只覆盖美股）；免费档 `commercial_use_allowed = false`，
//! 与 `calendar.rs`/`quote.rs` 的 finnhub 适配器同一授权口径。
//!
//! 只保留"实质性公司公告"表单类型（10-K/10-Q/8-K/proxy/registration 等）；内部人交易表单
//! （3/4/5/144）属另一类事实（持仓变动而非公司披露），本轮不纳入，避免把研究证据块淹没在
//! 高频交易噪音里。

use crate::{Market, detect_market, normalize_ticker};
use chrono::{DateTime, NaiveDate, Utc};
use echo_config::DataSourceConfig;
use echo_db::{FilingRow, FilingsRepository, NewFiling, Pool};
use serde::Deserialize;
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum FilingsError {
    #[error("FINNHUB_API_KEY 未配置")]
    MissingApiKey,
    #[error("商用模式不允许未授权的 Finnhub 免费源")]
    CommercialBlocked,
    #[error("公司公告仅支持美股（EDGAR 本身不覆盖港股/A股）：{0}")]
    UnsupportedMarket(String),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Db(#[from] echo_db::DbError),
}

/// 读库缓存超过此窗口视为过期，需重新向 Finnhub 取数。
const STALE_AFTER: chrono::Duration = chrono::Duration::hours(24);
/// 单次同步最多拉取的原始条目——Finnhub 默认按最新在前返回，足够覆盖近几个季度。
const FETCH_LIMIT: usize = 250;
/// 展示给研究链路的最近公告条数上限。
const DISPLAY_LIMIT: i64 = 8;
/// 实质性公司公告表单——排除内部人交易（3/4/5/144）与程序性文书。
const MATERIAL_FORMS: &[&str] = &[
    "10-K", "10-K/A", "10-Q", "10-Q/A", "8-K", "8-K/A", "DEF 14A", "DEFA14A", "S-1", "S-3",
    "S-3ASR", "S-8", "424B2", "424B5", "20-F", "6-K",
];

#[derive(Debug, Deserialize)]
struct RawFiling {
    form: String,
    #[serde(rename = "filedDate")]
    filed_date: Option<String>,
    #[serde(rename = "acceptedDate")]
    accepted_date: Option<String>,
    #[serde(rename = "reportUrl")]
    report_url: Option<String>,
    #[serde(rename = "filingUrl")]
    filing_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Filing {
    pub form: String,
    pub filed_date: Option<String>,
    pub source_url: String,
}

#[derive(Clone)]
pub struct FilingsService {
    client: reqwest::Client,
    pool: Pool,
    config: DataSourceConfig,
}

impl FilingsService {
    pub fn new(pool: Pool, config: DataSourceConfig) -> Result<Self, FilingsError> {
        let client = reqwest::Client::builder()
            .user_agent("EchoResearch/1.0")
            .timeout(Duration::from_secs(10))
            .build()?;
        Ok(Self {
            client,
            pool,
            config,
        })
    }

    /// 读库优先；从未同步或超过 [`STALE_AFTER`] 才回源并回写。非美股主体直接拒绝。
    pub async fn recent(&self, raw_ticker: &str) -> Vec<Filing> {
        let ticker = normalize_ticker(raw_ticker);
        if detect_market(&ticker) != Market::Us {
            return Vec::new();
        }
        let repo = FilingsRepository::new(&self.pool);
        let fresh = matches!(
            repo.last_synced(&ticker).await,
            Ok(Some(when)) if Utc::now() - when < STALE_AFTER
        );
        if !fresh {
            if let Err(error) = self.refresh(&ticker).await {
                tracing::warn!(ticker = %ticker, error = %error, "公司公告未核到");
            }
        }
        repo.recent(&ticker, DISPLAY_LIMIT)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(row_to_filing)
            .collect()
    }

    async fn refresh(&self, ticker: &str) -> Result<(), FilingsError> {
        if self.config.commercial_mode {
            return Err(FilingsError::CommercialBlocked);
        }
        let Some(api_key) = self.config.finnhub_api_key.as_deref() else {
            return Err(FilingsError::MissingApiKey);
        };
        if detect_market(ticker) != Market::Us {
            return Err(FilingsError::UnsupportedMarket(ticker.into()));
        }
        let raw: Vec<RawFiling> = self
            .client
            .get("https://finnhub.io/api/v1/stock/filings")
            .query(&[("symbol", ticker), ("token", api_key)])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let new_filings: Vec<NewFiling> = raw
            .into_iter()
            .take(FETCH_LIMIT)
            .filter(|item| MATERIAL_FORMS.contains(&item.form.as_str()))
            .filter_map(|item| {
                let filing_url = item.filing_url?;
                Some(NewFiling {
                    form: item.form,
                    filed_date: item.filed_date.as_deref().and_then(parse_date),
                    accepted_date: item.accepted_date.as_deref().and_then(parse_datetime),
                    report_url: item.report_url,
                    filing_url,
                })
            })
            .collect();

        FilingsRepository::new(&self.pool)
            .insert_batch(ticker, &new_filings)
            .await?;
        Ok(())
    }
}

fn row_to_filing(row: FilingRow) -> Filing {
    Filing {
        form: row.form,
        filed_date: row.filed_date.map(|date| date.to_string()),
        source_url: row.filing_url,
    }
}

/// Finnhub 日期格式固定 `YYYY-MM-DD HH:MM:SS`；只取日期部分。
fn parse_date(value: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(value.split(' ').next()?, "%Y-%m-%d").ok()
}

fn parse_datetime(value: &str) -> Option<DateTime<Utc>> {
    let naive = chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S").ok()?;
    Some(naive.and_utc())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_finnhub_date_and_datetime_formats() {
        assert_eq!(
            parse_date("2026-06-17 00:00:00"),
            NaiveDate::from_ymd_opt(2026, 6, 17)
        );
        assert_eq!(parse_date(""), None);
        let dt = parse_datetime("2026-06-17 18:40:43").expect("datetime");
        assert_eq!(dt.to_rfc3339(), "2026-06-17T18:40:43+00:00");
    }

    #[test]
    fn material_forms_exclude_insider_trading_noise() {
        assert!(MATERIAL_FORMS.contains(&"10-K"));
        assert!(MATERIAL_FORMS.contains(&"8-K"));
        assert!(!MATERIAL_FORMS.contains(&"4"));
        assert!(!MATERIAL_FORMS.contains(&"144"));
    }
}
