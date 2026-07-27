//! 同业锚点（comp_peers）——FMP `stock-peers` 选可比公司，逐个取 TTM PE / EV-Sales，
//! 按分位缓存进 `comp_peers`。美股专属：`ratios-ttm`/`key-metrics-ttm` 免费档对非美股
//! 代码返回 premium 错误体，与 `fundamentals.rs`/`historical.rs` 同一授权口径。
//!
//! 单个可比公司取数失败（不少小盘/非美股 ADR 命中 premium 门槛）不拖垮整批——诚实丢弃
//! 该公司，`partial` 标记本轮锚点不完整，绝不用陈旧/臆造倍数补位。

use crate::fmp::{FmpError, decimal_at, fetch_json, string_at};
use crate::{Market, detect_market, normalize_ticker};
use chrono::Utc;
use echo_config::DataSourceConfig;
use echo_db::{PeersRepository, PeersRow, PeersUpsert, Pool};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde_json::{Value, json};
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum PeersError {
    #[error("FMP_API_KEY 未配置")]
    MissingApiKey,
    #[error("商用模式不允许未授权的 FMP 免费源")]
    CommercialBlocked,
    #[error("同业锚点仅支持美股：{0}")]
    UnsupportedMarket(String),
    #[error(transparent)]
    Fmp(#[from] FmpError),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Db(#[from] echo_db::DbError),
}

/// 读库缓存超过此窗口视为过期，需重新取数。
const STALE_AFTER: chrono::Duration = chrono::Duration::hours(24);
/// 最多取几家可比公司做分位——超过意义有限，反而拖慢单轮研究。
const MAX_PEERS: usize = 8;
/// 少于两个有效倍数不成分位，诚实返回缺锚点。
const MIN_PEERS_FOR_BAND: usize = 2;

#[derive(Clone, Debug, PartialEq)]
pub struct PeerBand {
    pub p25: Decimal,
    pub median: Decimal,
    pub p75: Decimal,
    pub n: usize,
    pub tickers: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PeerSetSummary {
    pub pe: Option<PeerBand>,
    pub ev_sales: Option<PeerBand>,
}

#[derive(Clone)]
pub struct PeerService {
    client: reqwest::Client,
    pool: Pool,
    config: DataSourceConfig,
}

impl PeerService {
    pub fn new(pool: Pool, config: DataSourceConfig) -> Result<Self, PeersError> {
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

    /// 读库优先；缺行或超过 [`STALE_AFTER`] 才回源并回写。非美股主体直接拒绝，不读表里
    /// 可能是别的授权期留下的陈旧行冒充"支持"。
    pub async fn load(&self, raw_ticker: &str) -> Option<PeerSetSummary> {
        let ticker = normalize_ticker(raw_ticker);
        if detect_market(&ticker) != Market::Us {
            return None;
        }
        let repo = PeersRepository::new(&self.pool);
        if let Ok(Some(row)) = repo.latest(&ticker).await {
            if Utc::now() - row.knowledge_time < STALE_AFTER {
                return summary_from_row(&row);
            }
        }
        match self.refresh(&ticker).await {
            Ok(()) => repo
                .latest(&ticker)
                .await
                .ok()
                .flatten()
                .and_then(|r| summary_from_row(&r)),
            Err(error) => {
                tracing::warn!(ticker = %ticker, error = %error, "同业锚点未核到");
                repo.latest(&ticker)
                    .await
                    .ok()
                    .flatten()
                    .and_then(|r| summary_from_row(&r))
            }
        }
    }

    async fn refresh(&self, ticker: &str) -> Result<(), PeersError> {
        if self.config.commercial_mode {
            return Err(PeersError::CommercialBlocked);
        }
        let Some(api_key) = self.config.fmp_api_key.as_deref() else {
            return Err(PeersError::MissingApiKey);
        };
        if detect_market(ticker) != Market::Us {
            return Err(PeersError::UnsupportedMarket(ticker.into()));
        }
        let peers_path = format!("stock-peers?symbol={}", encode(ticker));
        let body = fetch_json(&self.client, api_key, &peers_path).await?;
        let peer_tickers = peer_symbols(&body, ticker);
        if peer_tickers.is_empty() {
            return Ok(());
        }

        let fetches = peer_tickers
            .iter()
            .map(|peer| self.fetch_multiples(api_key, peer));
        let results = futures_util::future::join_all(fetches).await;
        let mut pe_values: Vec<(String, Decimal)> = Vec::new();
        let mut ev_sales_values: Vec<(String, Decimal)> = Vec::new();
        let mut peers_json = Vec::new();
        for (peer, outcome) in peer_tickers.iter().zip(results.into_iter()) {
            let (pe, ev_sales) = outcome.unwrap_or((None, None));
            if let Some(pe) = pe.filter(|v| *v > Decimal::ZERO) {
                pe_values.push((peer.clone(), pe));
            }
            if let Some(ev_sales) = ev_sales.filter(|v| *v > Decimal::ZERO) {
                ev_sales_values.push((peer.clone(), ev_sales));
            }
            peers_json.push(json!({ "ticker": peer, "pe_ttm": pe, "ev_sales_ttm": ev_sales }));
        }

        let pe_band = band(pe_values);
        let ev_sales_band = band(ev_sales_values);
        let partial = peers_json.len() < peer_tickers.len()
            || pe_band.as_ref().is_none_or(|b| b.n < peer_tickers.len())
            || ev_sales_band
                .as_ref()
                .is_none_or(|b| b.n < peer_tickers.len());
        let anchor_json = json!({
            "pe": pe_band.as_ref().map(band_json),
            "ev_sales": ev_sales_band.as_ref().map(band_json),
        });

        PeersRepository::new(&self.pool)
            .upsert(&PeersUpsert {
                ticker: ticker.into(),
                peers_json: Value::Array(peers_json),
                anchor_json,
                partial,
            })
            .await?;
        Ok(())
    }

    /// 单个可比公司的 TTM PE / EV-Sales；任一失败（含 premium 门槛）诚实返回 `None`，
    /// 不拖垮整批取数。
    async fn fetch_multiples(
        &self,
        api_key: &str,
        peer_ticker: &str,
    ) -> Result<(Option<Decimal>, Option<Decimal>), PeersError> {
        let ratios_path = format!("ratios-ttm?symbol={}", encode(peer_ticker));
        let metrics_path = format!("key-metrics-ttm?symbol={}", encode(peer_ticker));
        let (ratios, metrics) = tokio::join!(
            fetch_json(&self.client, api_key, &ratios_path),
            fetch_json(&self.client, api_key, &metrics_path),
        );
        let pe = ratios
            .ok()
            .as_ref()
            .and_then(|body| body.as_array()?.first())
            .and_then(|row| decimal_at(row, "priceToEarningsRatioTTM"));
        let ev_sales = metrics
            .ok()
            .as_ref()
            .and_then(|body| body.as_array()?.first())
            .and_then(|row| decimal_at(row, "evToSalesTTM"));
        Ok((pe, ev_sales))
    }
}

fn summary_from_row(row: &PeersRow) -> Option<PeerSetSummary> {
    let anchor = row.anchor_json.as_ref()?;
    Some(PeerSetSummary {
        pe: band_from_json(anchor.get("pe")),
        ev_sales: band_from_json(anchor.get("ev_sales")),
    })
}

fn band_from_json(value: Option<&Value>) -> Option<PeerBand> {
    let value = value?;
    if value.is_null() {
        return None;
    }
    Some(PeerBand {
        p25: decimal_at(value, "p25")?,
        median: decimal_at(value, "median")?,
        p75: decimal_at(value, "p75")?,
        n: value.get("n")?.as_u64()? as usize,
        tickers: value
            .get("tickers")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
    })
}

fn band_json(band: &PeerBand) -> Value {
    json!({
        "p25": band.p25,
        "median": band.median,
        "p75": band.p75,
        "n": band.n,
        "tickers": band.tickers,
    })
}

/// 排序后取分位；少于 [`MIN_PEERS_FOR_BAND`] 个正值诚实返回 `None`，不凑数。
fn band(mut values: Vec<(String, Decimal)>) -> Option<PeerBand> {
    if values.len() < MIN_PEERS_FOR_BAND {
        return None;
    }
    values.sort_by(|a, b| a.1.cmp(&b.1));
    let n = values.len();
    // 线性插值分位（同 PostgreSQL `percentile_cont`）。此前是「四舍五入取最近索引」，在小样本
    // 上会让相邻分位塌到同一个值：n=3 时 p25 与中位同取 values[1]，n=4 时中位与 p75 同取
    // values[2]。免费档可比集恰好就是 3–5 家，于是同业带长期退化成一个点，看着像"同业口径
    // 高度一致"，实际是算法假象（实测 AAPL 4 家 → 中位 = p75 = 22.65x）。
    let at = |p: Decimal| -> Decimal {
        let pos = p * Decimal::from(n - 1);
        let lo_index = pos.floor().to_usize().unwrap_or(0).min(n - 1);
        let hi_index = pos.ceil().to_usize().unwrap_or(0).min(n - 1);
        let (lo, hi) = (values[lo_index].1, values[hi_index].1);
        if lo_index == hi_index {
            return lo;
        }
        lo + (hi - lo) * (pos - pos.floor())
    };
    Some(PeerBand {
        p25: at(Decimal::new(25, 2)),
        median: at(Decimal::new(50, 2)),
        p75: at(Decimal::new(75, 2)),
        n,
        tickers: values.into_iter().map(|(ticker, _)| ticker).collect(),
    })
}

fn peer_symbols(body: &Value, exclude: &str) -> Vec<String> {
    let Some(items) = body.as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| string_at(item, "symbol"))
        .map(|symbol| symbol.trim().to_ascii_uppercase())
        .filter(|symbol| !symbol.is_empty() && symbol != exclude)
        .take(MAX_PEERS)
        .collect()
}

fn encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;
    use serde_json::json;

    #[test]
    fn band_needs_at_least_two_positive_values() {
        assert!(band(vec![("A".into(), dec!(10))]).is_none());
    }

    /// 小样本不得让相邻分位塌到同一个值——那会把"可比公司太少"伪装成"同业估值高度一致"。
    /// 免费档可比集就是 3–5 家，取整分位在 n=3（p25==中位）和 n=4（中位==p75）时必然退化。
    #[test]
    fn small_peer_sets_still_spread_across_quartiles() {
        let four = band(vec![
            ("A".into(), dec!(10)),
            ("B".into(), dec!(20)),
            ("C".into(), dec!(30)),
            ("D".into(), dec!(40)),
        ])
        .expect("band");
        assert_eq!(four.p25, dec!(17.5));
        assert_eq!(four.median, dec!(25));
        assert_eq!(four.p75, dec!(32.5));
        assert!(four.p25 < four.median && four.median < four.p75);

        let three = band(vec![
            ("A".into(), dec!(10)),
            ("B".into(), dec!(20)),
            ("C".into(), dec!(30)),
        ])
        .expect("band");
        assert_eq!(three.p25, dec!(15));
        assert_eq!(three.median, dec!(20));
        assert_eq!(three.p75, dec!(25));
        assert!(three.p25 < three.median && three.median < three.p75);
    }

    #[test]
    fn band_computes_percentiles_on_sorted_values() {
        let values = vec![
            ("A".into(), dec!(10)),
            ("B".into(), dec!(20)),
            ("C".into(), dec!(30)),
            ("D".into(), dec!(40)),
            ("E".into(), dec!(50)),
        ];
        let result = band(values).expect("band");
        assert_eq!(result.n, 5);
        assert_eq!(result.median, dec!(30));
        assert_eq!(result.p25, dec!(20));
        assert_eq!(result.p75, dec!(40));
        assert_eq!(result.tickers, vec!["A", "B", "C", "D", "E"]);
    }

    #[test]
    fn peer_symbols_excludes_self_and_caps_at_max() {
        let body = json!([
            { "symbol": "AAPL" },
            { "symbol": "msft" },
            { "symbol": "googl" },
            { "symbol": "meta" },
            { "symbol": "nvda" },
            { "symbol": "tsm" },
            { "symbol": "sony" },
            { "symbol": "amzn" },
            { "symbol": "orcl" },
        ]);
        let symbols = peer_symbols(&body, "AAPL");
        assert_eq!(symbols.len(), MAX_PEERS);
        assert!(!symbols.contains(&"AAPL".to_string()));
        assert!(symbols.contains(&"MSFT".to_string()));
    }

    #[test]
    fn round_trip_summary_through_anchor_json() {
        let anchor = json!({
            "pe": { "p25": "20", "median": "25", "p75": "30", "n": 4, "tickers": ["A", "B", "C", "D"] },
            "ev_sales": Value::Null,
        });
        let row = PeersRow {
            ticker: "AAPL".into(),
            peers_json: None,
            anchor_json: Some(anchor),
            knowledge_time: Utc::now(),
        };
        let summary = summary_from_row(&row).expect("summary");
        assert_eq!(summary.pe.expect("pe band").median, dec!(25));
        assert!(summary.ev_sales.is_none());
    }
}
