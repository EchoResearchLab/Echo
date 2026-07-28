//! 网页证据检索——定性研究的二手来源入口。**双供应商可切换**：配了 `EXA_API_KEY` 走 Exa
//! （语义检索），否则回落 `TAVILY_API_KEY`；端口与主链路无关，换供应商只动本文件。
//!
//! 有数据库时优先读 `web_evidence` 24h 缓存，未命中才请求供应商并回写；无数据库时保持
//! 纯实时路径。**只做定性支撑**——返回的正文片段供模型引用并标注来源，绝不进
//! `FactsRegistry`、绝不当作已核财务数字来源（护栏仍只认一手财报）。
//!
//! 授权口径：免费/研究档不算商用授权，故 `commercial_mode` 下直接拒绝（返回空），与
//! filings/calendar 等免费源同一处理。任何网络/解析失败一律返回空列表，让研究链路诚实降级
//! 到"本轮无网页证据、置信度下降"，绝不把错误抛给主链路。

use crate::normalize_ticker;
use chrono::{DateTime, NaiveDate, Utc};
use echo_config::DataSourceConfig;
use echo_db::{NewWebEvidence, Pool, WebEvidenceRepository, WebEvidenceRow};
use rust_decimal::Decimal;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::time::Duration;

/// 单条片段最长保留字符数——控制注入提示词的体量，超长截断加省略号。
const SNIPPET_MAX_CHARS: usize = 500;
/// 单次检索返回的证据条数上限。
const MAX_RESULTS: u8 = 5;
/// 相同 ticker/provider/query 的缓存有效期。定性证据需要时效性，不能无限复用旧网页。
const CACHE_TTL: chrono::Duration = chrono::Duration::hours(24);
const EXA_SEARCH_URL: &str = "https://api.exa.ai/search";
const TAVILY_SEARCH_URL: &str = "https://api.tavily.com/search";

/// 一条网页证据（echo-data 边界类型；echo-api 映射为 `echo_domain::Evidence`，同 `filings::Filing`
/// 的映射惯例——本 crate 不依赖 echo-domain）。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Evidence {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub published_date: Option<String>,
    pub source_domain: Option<String>,
}

/// 证据供应商——配了 Exa key 优先 Exa，否则 Tavily。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvidenceProvider {
    Exa,
    Tavily,
}

impl EvidenceProvider {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Exa => "exa",
            Self::Tavily => "tavily",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EvidenceError {
    #[error("未配置证据供应商 key（EXA_API_KEY 或 TAVILY_API_KEY）")]
    MissingApiKey,
    #[error("商用模式不允许未授权的免费证据源")]
    CommercialBlocked,
    #[error(transparent)]
    Http(#[from] reqwest::Error),
}

#[derive(Clone)]
pub struct EvidenceService {
    client: reqwest::Client,
    provider: Option<EvidenceProvider>,
    api_key: Option<String>,
    commercial_mode: bool,
    pool: Option<Pool>,
}

impl EvidenceService {
    pub fn new(config: DataSourceConfig) -> Result<Self, EvidenceError> {
        let client = reqwest::Client::builder()
            .user_agent("EchoResearch/1.0")
            .timeout(Duration::from_secs(12))
            .build()?;
        let (provider, api_key) = resolve_provider(&config);
        Ok(Self {
            client,
            provider,
            api_key,
            commercial_mode: config.commercial_mode,
            pool: None,
        })
    }

    /// 生产组合根使用：在实时供应商前接 PostgreSQL 缓存。构造仍不访问数据库，库故障只会在
    /// 单次检索时降级为实时请求，不影响服务启动。
    pub fn new_cached(pool: Pool, config: DataSourceConfig) -> Result<Self, EvidenceError> {
        let mut service = Self::new(config)?;
        service.pool = Some(pool);
        Ok(service)
    }

    /// 配了任一供应商 key 且非商用模式才算可用；供调用方在纯核/未配场景下跳过。
    #[must_use]
    pub fn is_configured(&self) -> bool {
        self.provider.is_some() && !self.commercial_mode
    }

    /// 检索一家公司在某问题下的网页证据。失败/未配一律返回空列表（诚实降级）。
    pub async fn search(&self, ticker: &str, name: Option<&str>, question: &str) -> Vec<Evidence> {
        match self.try_search(ticker, name, question).await {
            Ok(evidence) => evidence,
            Err(error) => {
                tracing::warn!(ticker, error = %error, "网页证据未核到");
                Vec::new()
            }
        }
    }

    async fn try_search(
        &self,
        ticker: &str,
        name: Option<&str>,
        question: &str,
    ) -> Result<Vec<Evidence>, EvidenceError> {
        if self.commercial_mode {
            return Err(EvidenceError::CommercialBlocked);
        }
        let (Some(provider), Some(api_key)) = (self.provider, self.api_key.as_deref()) else {
            return Err(EvidenceError::MissingApiKey);
        };
        let ticker = normalize_ticker(ticker);
        let query = build_query(&ticker, name, question);
        if let Some(cached) = self
            .read_cache(&ticker, &query, provider, Some(Utc::now() - CACHE_TTL))
            .await
        {
            return Ok(cached);
        }
        let hits = match provider {
            EvidenceProvider::Exa => self.fetch_exa(api_key, &query).await,
            EvidenceProvider::Tavily => self.fetch_tavily(api_key, &query).await,
        };
        let hits = match hits {
            Ok(hits) => hits,
            Err(error) => {
                // 回源失败时允许用同检索式的陈旧缓存兜底，并保留原 `published_date`，调用方仍能
                // 判断来源时间；没有缓存才把供应商错误交给诚实降级路径。
                if let Some(stale) = self.read_cache(&ticker, &query, provider, None).await {
                    return Ok(stale);
                }
                return Err(error);
            }
        };
        let evidence: Vec<Evidence> = hits.into_iter().filter_map(normalize_hit).collect();
        if evidence.is_empty() {
            if let Some(stale) = self.read_cache(&ticker, &query, provider, None).await {
                return Ok(stale);
            }
        }
        self.write_cache(&ticker, &query, provider, &evidence).await;
        Ok(evidence)
    }

    async fn read_cache(
        &self,
        ticker: &str,
        query: &str,
        provider: EvidenceProvider,
        fresh_after: Option<DateTime<Utc>>,
    ) -> Option<Vec<Evidence>> {
        let pool = self.pool.as_ref()?;
        match WebEvidenceRepository::new(pool)
            .cached(
                ticker,
                query,
                provider.as_str(),
                fresh_after,
                i64::from(MAX_RESULTS),
            )
            .await
        {
            Ok(rows) if !rows.is_empty() => Some(rows.into_iter().map(cached_evidence).collect()),
            Ok(_) => None,
            Err(error) => {
                tracing::warn!(ticker, error = %error, "读取网页证据缓存失败，回落实时供应商");
                None
            }
        }
    }

    async fn write_cache(
        &self,
        ticker: &str,
        query: &str,
        provider: EvidenceProvider,
        evidence: &[Evidence],
    ) {
        let (Some(pool), false) = (self.pool.as_ref(), evidence.is_empty()) else {
            return;
        };
        let rows = evidence
            .iter()
            .enumerate()
            .map(|(index, item)| NewWebEvidence {
                id: cache_row_id(ticker, provider, query, &item.url),
                ticker: ticker.to_string(),
                query: query.to_string(),
                provider: provider.as_str().to_string(),
                title: item.title.clone(),
                url: item.url.clone(),
                source_domain: item.source_domain.clone(),
                snippet: item.snippet.clone(),
                published_at: item.published_date.as_deref().and_then(parse_published_at),
                relevance_score: Decimal::from(i64::from(MAX_RESULTS) - index as i64),
            })
            .collect::<Vec<_>>();
        if let Err(error) = WebEvidenceRepository::new(pool).replace(&rows).await {
            // 常见原因是公司尚未建档触发 FK；缓存失败不能吃掉本轮已经拿到的实时证据。
            tracing::warn!(ticker, error = %error, "写入网页证据缓存失败，本轮继续使用实时结果");
        }
    }

    /// Exa `/search`：`x-api-key` 头鉴权，合并 contents 一次取回正文（`text.maxCharacters`）。
    async fn fetch_exa(&self, api_key: &str, query: &str) -> Result<Vec<RawHit>, EvidenceError> {
        let body = serde_json::json!({
            "query": query,
            "type": "auto",
            "numResults": MAX_RESULTS,
            "contents": { "text": { "maxCharacters": SNIPPET_MAX_CHARS * 3 } },
        });
        let response: ExaResponse = self
            .client
            .post(EXA_SEARCH_URL)
            .header("x-api-key", api_key)
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(response
            .results
            .into_iter()
            .map(|r| RawHit {
                title: r.title.unwrap_or_default(),
                url: r.url,
                text: r.text.unwrap_or_default(),
                published_date: r.published_date,
            })
            .collect())
    }

    /// Tavily `/search`：Bearer 鉴权，advanced 深度，正文在 `content`。
    async fn fetch_tavily(&self, api_key: &str, query: &str) -> Result<Vec<RawHit>, EvidenceError> {
        let body = serde_json::json!({
            "query": query,
            "search_depth": "advanced",
            "max_results": MAX_RESULTS,
            "include_answer": false,
        });
        let response: TavilyResponse = self
            .client
            .post(TAVILY_SEARCH_URL)
            .bearer_auth(api_key)
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(response
            .results
            .into_iter()
            .map(|r| RawHit {
                title: r.title,
                url: r.url,
                text: r.content,
                published_date: r.published_date,
            })
            .collect())
    }
}

/// 供应商解析：Exa key 优先，否则 Tavily，都没有则无供应商。
fn resolve_provider(config: &DataSourceConfig) -> (Option<EvidenceProvider>, Option<String>) {
    if let Some(key) = config.exa_api_key.clone() {
        return (Some(EvidenceProvider::Exa), Some(key));
    }
    if let Some(key) = config.tavily_api_key.clone() {
        return (Some(EvidenceProvider::Tavily), Some(key));
    }
    (None, None)
}

/// 供应商无关的原始命中——两家响应各自映射到这里，再统一 `normalize_hit`。
struct RawHit {
    title: String,
    url: String,
    text: String,
    published_date: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ExaResponse {
    #[serde(default)]
    results: Vec<ExaResult>,
}

#[derive(Debug, Deserialize)]
struct ExaResult {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    url: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default, rename = "publishedDate")]
    published_date: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TavilyResponse {
    #[serde(default)]
    results: Vec<TavilyResult>,
}

#[derive(Debug, Deserialize)]
struct TavilyResult {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    published_date: Option<String>,
}

/// 拼检索式：公司名（若已核到）+ 代码 + 用户问题。名字命中显著提升中文/港股结果质量。
fn build_query(ticker: &str, name: Option<&str>, question: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if let Some(name) = name.map(str::trim).filter(|n| !n.is_empty()) {
        parts.push(name);
    }
    let ticker = ticker.trim();
    if !ticker.is_empty() {
        parts.push(ticker);
    }
    let question = question.trim();
    if !question.is_empty() {
        parts.push(question);
    }
    parts.join(" ")
}

/// 空 url 视为无效丢弃；正文截断；域名从 url 解析（去 `www.`）。
fn normalize_hit(hit: RawHit) -> Option<Evidence> {
    let url = hit.url.trim().to_string();
    if url.is_empty() {
        return None;
    }
    let source_domain = domain_of(&url);
    Some(Evidence {
        title: hit.title.trim().to_string(),
        snippet: truncate_chars(hit.text.trim(), SNIPPET_MAX_CHARS),
        published_date: hit
            .published_date
            .map(|d| d.trim().to_string())
            .filter(|d| !d.is_empty()),
        source_domain,
        url,
    })
}

fn domain_of(url: &str) -> Option<String> {
    let host = url::Url::parse(url).ok()?.host_str()?.to_string();
    Some(host.strip_prefix("www.").unwrap_or(&host).to_string())
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

fn cached_evidence(row: WebEvidenceRow) -> Evidence {
    Evidence {
        title: row.title.unwrap_or_default(),
        url: row.url,
        snippet: row.snippet.unwrap_or_default(),
        published_date: row
            .valid_time
            .map(|date| date.format("%Y-%m-%d").to_string()),
        source_domain: row.source_type,
    }
}

fn parse_published_at(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|date| date.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .ok()?
                .and_hms_opt(0, 0, 0)
                .map(|date| date.and_utc())
        })
}

fn cache_row_id(ticker: &str, provider: EvidenceProvider, query: &str, url: &str) -> String {
    let digest =
        Sha256::digest(format!("{ticker}\n{}\n{query}\n{url}", provider.as_str()).as_bytes());
    format!("web:{}", hex::encode(digest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_prefers_exa_then_tavily_then_none() {
        let both = DataSourceConfig {
            exa_api_key: Some("exa-x".into()),
            tavily_api_key: Some("tvly-x".into()),
            ..Default::default()
        };
        assert_eq!(resolve_provider(&both).0, Some(EvidenceProvider::Exa));
        let tavily_only = DataSourceConfig {
            tavily_api_key: Some("tvly-x".into()),
            ..Default::default()
        };
        assert_eq!(
            resolve_provider(&tavily_only).0,
            Some(EvidenceProvider::Tavily)
        );
        assert_eq!(resolve_provider(&DataSourceConfig::default()).0, None);
    }

    #[test]
    fn query_joins_name_ticker_question_and_skips_blanks() {
        assert_eq!(
            build_query("AAPL", Some("苹果"), "护城河在哪"),
            "苹果 AAPL 护城河在哪"
        );
        assert_eq!(build_query("AAPL", None, "为什么跌"), "AAPL 为什么跌");
        assert_eq!(build_query("AAPL", Some("  "), " "), "AAPL");
    }

    #[test]
    fn normalize_drops_empty_url_and_extracts_domain() {
        assert!(
            normalize_hit(RawHit {
                title: "t".into(),
                url: "  ".into(),
                text: "c".into(),
                published_date: None,
            })
            .is_none()
        );
        let ev = normalize_hit(RawHit {
            title: "  Apple moat  ".into(),
            url: "https://www.reuters.com/tech/apple".into(),
            text: "  body  ".into(),
            published_date: Some(" 2026-07-01 ".into()),
        })
        .expect("evidence");
        assert_eq!(ev.title, "Apple moat");
        assert_eq!(ev.snippet, "body");
        assert_eq!(ev.source_domain.as_deref(), Some("reuters.com"));
        assert_eq!(ev.published_date.as_deref(), Some("2026-07-01"));
    }

    #[test]
    fn snippet_truncates_on_char_boundary() {
        let long = "壁".repeat(600);
        let ev = normalize_hit(RawHit {
            title: "t".into(),
            url: "https://x.com/a".into(),
            text: long,
            published_date: None,
        })
        .expect("evidence");
        assert_eq!(ev.snippet.chars().count(), SNIPPET_MAX_CHARS + 1); // 500 + 省略号
        assert!(ev.snippet.ends_with('…'));
    }

    #[test]
    fn cache_identity_is_deterministic_and_provider_isolated() {
        let exa = cache_row_id(
            "AAPL",
            EvidenceProvider::Exa,
            "苹果 AAPL 护城河",
            "https://example.com/a",
        );
        assert_eq!(
            exa,
            cache_row_id(
                "AAPL",
                EvidenceProvider::Exa,
                "苹果 AAPL 护城河",
                "https://example.com/a"
            )
        );
        assert_ne!(
            exa,
            cache_row_id(
                "AAPL",
                EvidenceProvider::Tavily,
                "苹果 AAPL 护城河",
                "https://example.com/a"
            )
        );
        assert_eq!(exa.len(), 68); // "web:" + SHA-256 hex
    }

    #[test]
    fn published_dates_round_trip_to_cache_timestamp() {
        assert_eq!(
            parse_published_at("2026-07-01")
                .expect("date")
                .format("%Y-%m-%d")
                .to_string(),
            "2026-07-01"
        );
        assert!(parse_published_at("not-a-date").is_none());
    }

    #[test]
    fn exa_and_tavily_response_shapes_both_parse() {
        let exa: ExaResponse = serde_json::from_value(serde_json::json!({
            "results": [{
                "title": "Apple moat",
                "url": "https://reuters.com/a",
                "text": "服务收入占比提升",
                "publishedDate": "2026-07-01"
            }]
        }))
        .expect("exa");
        assert_eq!(exa.results[0].url, "https://reuters.com/a");
        assert_eq!(exa.results[0].published_date.as_deref(), Some("2026-07-01"));
        let tavily: TavilyResponse = serde_json::from_value(serde_json::json!({
            "results": [{
                "title": "Apple moat",
                "url": "https://reuters.com/a",
                "content": "服务收入占比提升",
                "published_date": "2026-07-01"
            }]
        }))
        .expect("tavily");
        assert_eq!(tavily.results[0].content, "服务收入占比提升");
    }

    #[tokio::test]
    async fn commercial_mode_returns_empty() {
        let service = EvidenceService::new(DataSourceConfig {
            commercial_mode: true,
            exa_api_key: Some("exa-x".into()),
            ..Default::default()
        })
        .expect("service");
        assert!(!service.is_configured());
        assert!(
            service
                .search("AAPL", Some("苹果"), "护城河")
                .await
                .is_empty()
        );
    }

    #[tokio::test]
    async fn missing_key_returns_empty() {
        let service = EvidenceService::new(DataSourceConfig::default()).expect("service");
        assert!(!service.is_configured());
        assert!(service.search("AAPL", None, "护城河").await.is_empty());
    }
}
