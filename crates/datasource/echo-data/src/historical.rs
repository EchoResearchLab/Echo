//! 历史估值分位——美股专属（勘察结论见 `docs/PLAN.md` 历史：港股 filing 年度 EPS
//! 深度只有 1-3 期，撑不起 5 年序列；美股靠 FMP 年度 EPS 撑满）。
//!
//! 逐月历史 PE 用**当时已知**的最近一期年度 EPS（按 `filingDate` 截止）计算，
//! 不用最新 EPS 反推过去价格——那是未来数据泄漏到历史，会把分位算错。

use crate::fmp::{FmpError, decimal_at, fetch_json, string_at};
use crate::{Market, detect_market, normalize_ticker};
use chrono::{NaiveDate, TimeZone, Utc};
use echo_config::DataSourceConfig;
use echo_db::{
    HistoricalValuationPointRow, HistoricalValuationRepository, HistoricalValuationWrite, Pool,
};
use rust_decimal::Decimal;
use serde_json::Value;
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum HistoricalValuationError {
    #[error("FMP_API_KEY 未配置")]
    MissingApiKey,
    #[error("商用模式不允许未授权的 FMP 免费源")]
    CommercialBlocked,
    #[error("历史估值分位仅支持美股（港股 filing EPS 深度不足）：{0}")]
    UnsupportedMarket(String),
    #[error("Yahoo 历史行情未核到：{0}")]
    ProvidersFailed(String),
    #[error(transparent)]
    Fmp(#[from] FmpError),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Db(#[from] echo_db::DbError),
}

/// 读库缓存超过此窗口视为过期，需重新取数（月度序列没必要天天刷）。
const STALE_AFTER: chrono::Duration = chrono::Duration::hours(24 * 7);

#[derive(Clone, Debug, Default, PartialEq)]
pub struct HistoricalValuationSummary {
    pub percentile: Option<Decimal>,
    pub min: Option<Decimal>,
    pub max: Option<Decimal>,
    pub median: Option<Decimal>,
    /// 四分位——估值 PE 法的熊/牛锚点。刻意不用 `min`/`max`：单个极端月份会毁掉整条带
    /// （NVDA 五年 PE 的 max 是 465x，落在 AI 周期起点的 EPS 低位上，当牛市情景毫无意义）。
    pub p25: Option<Decimal>,
    pub p75: Option<Decimal>,
    /// 序列最新一点的 PE，与 `min`/`median`/`p25`/`p75` **同口径**（都用当时已知的最近一期
    /// 年报 EPS）。估值只能用「分位 ÷ latest」这个比值，绝不能把本序列的绝对倍数去乘 TTM EPS：
    /// 两者 EPS 口径不同，实测 GOOGL 年报口径 29.58x vs TTM 口径 15.88x，差 86%。
    pub latest: Option<Decimal>,
}

#[derive(Clone)]
pub struct HistoricalValuationService {
    client: reqwest::Client,
    pool: Pool,
    config: DataSourceConfig,
}

struct AnnualEps {
    filing_date: NaiveDate,
    eps: Decimal,
}

impl HistoricalValuationService {
    pub fn new(pool: Pool, config: DataSourceConfig) -> Result<Self, HistoricalValuationError> {
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

    /// 读库优先；缺行或超过 [`STALE_AFTER`] 才回源并回写。失败即诚实返回 `None`。
    /// 不支持的市场（港股/A股，勘察结论见 `docs/PLAN.md` 历史）直接拒绝，绝不读表里
    /// 可能是别的口径/别的时间留下的陈旧点位来冒充"支持"。
    pub async fn load(&self, raw_ticker: &str) -> Option<HistoricalValuationSummary> {
        let ticker = normalize_ticker(raw_ticker);
        if detect_market(&ticker) != Market::Us {
            return None;
        }
        let repo = HistoricalValuationRepository::new(&self.pool);
        let fresh = matches!(
            repo.knowledge_time(&ticker).await,
            Ok(Some(when)) if Utc::now() - when < STALE_AFTER
        );
        if !fresh {
            if let Err(error) = self.refresh(&ticker).await {
                tracing::warn!(ticker = %ticker, error = %error, "历史估值分位未核到");
            }
        }
        let points = repo.points(&ticker).await.ok()?;
        summarize(&points)
    }

    async fn refresh(&self, ticker: &str) -> Result<(), HistoricalValuationError> {
        if self.config.commercial_mode {
            return Err(HistoricalValuationError::CommercialBlocked);
        }
        let Some(api_key) = self.config.fmp_api_key.as_deref() else {
            return Err(HistoricalValuationError::MissingApiKey);
        };
        if detect_market(ticker) != Market::Us {
            return Err(HistoricalValuationError::UnsupportedMarket(ticker.into()));
        }
        let eps_series = self.fetch_annual_eps(api_key, ticker).await?;
        if eps_series.is_empty() {
            return Ok(());
        }
        let closes = self.fetch_monthly_closes(ticker).await?;
        let points: Vec<(String, Decimal)> = closes
            .into_iter()
            .filter_map(|(date, close)| {
                let eps = eps_as_of(&eps_series, date)?;
                (eps > Decimal::ZERO).then(|| (date.to_string(), (close / eps).round_dp(4)))
            })
            .collect();
        if points.is_empty() {
            return Ok(());
        }
        HistoricalValuationRepository::new(&self.pool)
            .write(&HistoricalValuationWrite {
                ticker: ticker.into(),
                points,
            })
            .await?;
        Ok(())
    }

    async fn fetch_annual_eps(
        &self,
        api_key: &str,
        ticker: &str,
    ) -> Result<Vec<AnnualEps>, HistoricalValuationError> {
        // 免费档 `limit` 上限为 5（超过返回纯文本错误体，不是 JSON）。
        let path = format!(
            "income-statement?symbol={}&period=annual&limit=5",
            encode(ticker)
        );
        let body = fetch_json(&self.client, api_key, &path).await?;
        let Some(rows) = body.as_array() else {
            return Ok(Vec::new());
        };
        let mut series: Vec<AnnualEps> = rows
            .iter()
            .filter_map(|row| {
                let filing_date = string_at(row, "filingDate")
                    .or_else(|| string_at(row, "date"))
                    .and_then(|value| NaiveDate::parse_from_str(&value, "%Y-%m-%d").ok())?;
                let eps = decimal_at(row, "epsDiluted").or_else(|| decimal_at(row, "eps"))?;
                Some(AnnualEps { filing_date, eps })
            })
            .collect();
        series.sort_by_key(|row| row.filing_date);
        Ok(series)
    }

    async fn fetch_monthly_closes(
        &self,
        ticker: &str,
    ) -> Result<Vec<(NaiveDate, Decimal)>, HistoricalValuationError> {
        let mut last_error = None;
        for host in ["query2.finance.yahoo.com", "query1.finance.yahoo.com"] {
            let url = format!("https://{host}/v8/finance/chart/{ticker}");
            let response = self
                .client
                .get(url)
                .query(&[("range", "5y"), ("interval", "1mo")])
                .send()
                .await;
            match response {
                Ok(response) if response.status().is_success() => {
                    let body: Value = response.json().await?;
                    return Ok(yahoo_monthly_closes(&body));
                }
                Ok(response) => last_error = Some(format!("HTTP {}", response.status())),
                Err(error) => last_error = Some(error.to_string()),
            }
        }
        Err(HistoricalValuationError::ProvidersFailed(
            last_error.unwrap_or_else(|| "Yahoo 无响应".into()),
        ))
    }
}

/// 截止 `as_of` 已知的最近一期年度 EPS——绝不用晚于该日期的年报反推更早的价格。
fn eps_as_of(series: &[AnnualEps], as_of: NaiveDate) -> Option<Decimal> {
    series
        .iter()
        .rev()
        .find(|row| row.filing_date <= as_of)
        .map(|row| row.eps)
}

fn yahoo_monthly_closes(body: &Value) -> Vec<(NaiveDate, Decimal)> {
    let result = body
        .get("chart")
        .and_then(|c| c.get("result"))
        .and_then(Value::as_array)
        .and_then(|results| results.first());
    let Some(result) = result else {
        return Vec::new();
    };
    let timestamps = result.get("timestamp").and_then(Value::as_array);
    let closes = result
        .get("indicators")
        .and_then(|i| i.get("quote"))
        .and_then(Value::as_array)
        .and_then(|values| values.first())
        .and_then(|q| q.get("close"))
        .and_then(Value::as_array);
    let (Some(timestamps), Some(closes)) = (timestamps, closes) else {
        return Vec::new();
    };
    timestamps
        .iter()
        .zip(closes.iter())
        .filter_map(|(ts, close)| {
            let seconds = ts.as_i64()?;
            let date = Utc.timestamp_opt(seconds, 0).single()?.date_naive();
            let close = close
                .as_f64()
                .and_then(|value| Decimal::try_from(value).ok())?;
            Some((date, close))
        })
        .collect()
}

fn summarize(points: &[HistoricalValuationPointRow]) -> Option<HistoricalValuationSummary> {
    let mut values: Vec<Decimal> = points.iter().filter_map(|p| p.pe_value).collect();
    if values.is_empty() {
        return None;
    }
    values.sort();
    let min = values.first().copied();
    let max = values.last().copied();
    // 与同业锚点共用同一套插值分位，否则同一条 PE 序列会在事实块与估值里给出不同的中位数。
    let (p25, median, p75) = match crate::stats::quartiles_sorted(&values) {
        Some((p25, median, p75)) => (Some(p25), Some(median), Some(p75)),
        None => (None, None, None),
    };
    // 分位用点位序列里最新一条（`points` 按日期升序）近似"当前"——月度粒度，非当日估值。
    let latest = points.last().and_then(|p| p.pe_value);
    let percentile = latest.map(|latest| {
        let below = values.iter().filter(|value| **value <= latest).count();
        Decimal::from(below) * Decimal::from(100) / Decimal::from(values.len())
    });
    Some(HistoricalValuationSummary {
        percentile: percentile.map(round2),
        min: min.map(round2),
        max: max.map(round2),
        median: median.map(round2),
        p25: p25.map(round2),
        p75: p75.map(round2),
        latest: latest.map(round2),
    })
}

fn round2(value: Decimal) -> Decimal {
    value.round_dp(2)
}

fn encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}
