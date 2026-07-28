//! FMP `stable` 代码/名称搜索——公司解析候选池，不单独做结论。
//!
//! - `search-symbol`：精确代码命中，免费档可用。
//! - `search-name`：模糊结果可能含多交易所挂牌；调用方必须过滤交易所并探活验证。
//! - 中文名基本不命中；失败返回 `[]`，不抛给研究主链路。

use crate::fmp::{self, FmpError, fetch_json, string_at};
use echo_config::DataSourceConfig;
use serde_json::Value;

/// 美股主板（含 ETF 主场所）。FMP 会吐全球挂牌，候选只认这些。
pub const US_MAIN_EXCHANGES: &[&str] = &["NASDAQ", "NYSE", "AMEX", "NYSE AMERICAN", "CBOE"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FmpSymbolHit {
    pub symbol: String,
    pub name: String,
    pub exchange: Option<String>,
    pub currency: Option<String>,
}

pub type SearchError = FmpError;

#[derive(Clone)]
pub struct FmpSearchService {
    client: reqwest::Client,
    config: DataSourceConfig,
}

impl FmpSearchService {
    pub fn new(config: DataSourceConfig) -> Result<Self, SearchError> {
        Ok(Self {
            client: fmp::build_client()?,
            config,
        })
    }

    #[must_use]
    pub fn is_configured(&self) -> bool {
        self.config.fmp_api_key.is_some() && !self.config.commercial_mode
    }

    pub async fn search_symbol(&self, symbol: &str) -> Vec<FmpSymbolHit> {
        let query = symbol.trim();
        if query.is_empty() || !self.is_configured() {
            return Vec::new();
        }
        let Some(api_key) = self.config.fmp_api_key.as_deref() else {
            return Vec::new();
        };
        let path = format!(
            "search-symbol?query={}&limit=8",
            url::form_urlencoded::byte_serialize(query.as_bytes()).collect::<String>()
        );
        match fetch_json(&self.client, api_key, &path).await {
            Ok(body) => normalize_hits(&body),
            Err(error) => {
                tracing::warn!(symbol = query, error = %error, "FMP search-symbol 失败");
                Vec::new()
            }
        }
    }

    pub async fn search_name(&self, name: &str) -> Vec<FmpSymbolHit> {
        let query = name.trim();
        if query.is_empty() || !self.is_configured() {
            return Vec::new();
        }
        let Some(api_key) = self.config.fmp_api_key.as_deref() else {
            return Vec::new();
        };
        let path = format!(
            "search-name?query={}&limit=10",
            url::form_urlencoded::byte_serialize(query.as_bytes()).collect::<String>()
        );
        match fetch_json(&self.client, api_key, &path).await {
            Ok(body) => normalize_hits(&body),
            Err(error) => {
                tracing::warn!(name = query, error = %error, "FMP search-name 失败");
                Vec::new()
            }
        }
    }

    /// 精确代码 + 美股主板过滤；未命中返回 `None`。
    pub async fn exact_us_hit(&self, ticker: &str) -> Option<FmpSymbolHit> {
        let symbol = ticker.trim().to_ascii_uppercase();
        self.search_symbol(&symbol).await.into_iter().find(|hit| {
            hit.symbol == symbol
                && hit
                    .exchange
                    .as_deref()
                    .map(is_us_main_exchange)
                    .unwrap_or(true)
        })
    }
}

#[must_use]
pub fn is_us_main_exchange(exchange: &str) -> bool {
    let upper = exchange.trim().to_ascii_uppercase();
    US_MAIN_EXCHANGES.iter().any(|item| *item == upper)
}

#[must_use]
pub fn best_us_name_hit(hits: &[FmpSymbolHit]) -> Option<&FmpSymbolHit> {
    hits.iter()
        .find(|hit| hit.exchange.as_deref().is_some_and(is_us_main_exchange))
}

fn normalize_hits(body: &Value) -> Vec<FmpSymbolHit> {
    let Some(items) = body.as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let symbol = string_at(item, "symbol")?.trim().to_ascii_uppercase();
            if symbol.is_empty() {
                return None;
            }
            Some(FmpSymbolHit {
                symbol,
                name: string_at(item, "name")
                    .or_else(|| string_at(item, "companyName"))
                    .unwrap_or_default(),
                exchange: string_at(item, "exchangeShortName")
                    .or_else(|| string_at(item, "exchange")),
                currency: string_at(item, "currency"),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn normalizes_and_filters_us_main() {
        let body = json!([
            {
                "symbol": "tcehy",
                "name": "Tencent ADR",
                "exchangeShortName": "OTC",
                "currency": "USD"
            },
            {
                "symbol": "aapl",
                "companyName": "Apple Inc.",
                "exchange": "NASDAQ",
                "currency": "USD"
            }
        ]);
        let hits = normalize_hits(&body);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].symbol, "TCEHY");
        assert_eq!(
            best_us_name_hit(&hits).map(|hit| hit.symbol.as_str()),
            Some("AAPL")
        );
        assert!(is_us_main_exchange("nyse american"));
    }

    #[tokio::test]
    async fn commercial_mode_returns_empty() {
        let service = FmpSearchService::new(DataSourceConfig {
            commercial_mode: true,
            fmp_api_key: Some("x".into()),
            ..Default::default()
        })
        .expect("service");
        assert!(!service.is_configured());
        assert!(service.search_symbol("AAPL").await.is_empty());
    }
}
