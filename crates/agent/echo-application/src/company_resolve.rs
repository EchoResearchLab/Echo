//! 公司解析 / 验证用例。
//!
//! 顺序对齐旧 `companyResolution.ts`：显式港股代码 → 别名 → DB → 美股词元 → FMP 名称搜索。
//! 研究入口经 [`CompanyResolveService::resolve_research_company`] 验证后，由 API 调用
//! `CompanyRepository::ensure` 建档（验证先于建档）。本切片不含 LLM 兜底。

use echo_domain::{
    extract_hk_ticker, extract_us_ticker_token, match_hk_alias, match_us_alias,
    normalize_question_text,
};
use std::future::Future;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedListing {
    pub ticker: String,
    pub name_zh: String,
    pub name_en: String,
    pub industry: String,
    pub source: ResolveSource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolveSource {
    Db,
    Alias,
    Fmp,
    Probe,
}

impl ResolveSource {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Db => "db",
            Self::Alias => "alias",
            Self::Fmp => "fmp",
            Self::Probe => "probe",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolveSuggestion {
    pub ticker: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VerifyStatus {
    Verified,
    NotFound,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifyResult {
    pub status: VerifyStatus,
    pub name: Option<String>,
    pub suggestions: Vec<ResolveSuggestion>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolveResult {
    pub company: Option<ResolvedListing>,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DbCompanyHit {
    pub ticker: String,
    pub name_zh: String,
    pub name_en: Option<String>,
    pub industry: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalSymbolHit {
    pub symbol: String,
    pub name: String,
    pub exchange: Option<String>,
}

/// 解析副作用端口——DB / FMP / 行情探活由 API 边界注入。
pub trait CompanyResolvePorts: Send + Sync {
    fn db_by_ticker(&self, ticker: &str) -> impl Future<Output = Option<DbCompanyHit>> + Send;

    fn db_search(&self, query: &str, limit: i64) -> impl Future<Output = Vec<DbCompanyHit>> + Send;

    fn fmp_exact_us(&self, ticker: &str) -> impl Future<Output = Option<ExternalSymbolHit>> + Send;

    fn fmp_search_name(&self, name: &str) -> impl Future<Output = Vec<ExternalSymbolHit>> + Send;

    fn quote_alive(&self, ticker: &str) -> impl Future<Output = bool> + Send;
}

#[derive(Clone, Debug, Default)]
pub struct CompanyResolveService;

impl CompanyResolveService {
    pub async fn resolve_query<P: CompanyResolvePorts>(ports: &P, query: &str) -> ResolveResult {
        let q = normalize_question_text(query);
        let q = q.trim();
        if q.chars().count() < 2 {
            return ResolveResult {
                company: None,
                reason: Some("empty".into()),
            };
        }

        if let Some(hk) = extract_hk_ticker(q) {
            return match Self::verify_candidate(ports, &hk, None, None).await {
                Some(company) => ResolveResult {
                    company: Some(company),
                    reason: None,
                },
                None => ResolveResult {
                    company: None,
                    reason: Some("hk_not_found".into()),
                },
            };
        }

        if let Some(alias) = match_hk_alias(q) {
            if let Some(mut company) = Self::verify_candidate(ports, alias.ticker, None, None).await
            {
                company.source = ResolveSource::Alias;
                return ResolveResult {
                    company: Some(company),
                    reason: None,
                };
            }
        }
        if let Some(alias) = match_us_alias(q) {
            if let Some(mut company) =
                Self::verify_candidate(ports, alias.ticker, alias.name, alias.name).await
            {
                company.source = ResolveSource::Alias;
                return ResolveResult {
                    company: Some(company),
                    reason: None,
                };
            }
        }

        if let Some(direct) = ports.db_by_ticker(q).await {
            return ResolveResult {
                company: Some(from_db(direct)),
                reason: None,
            };
        }
        if let Some(hit) = ports.db_search(q, 1).await.into_iter().next() {
            if let Some(complete) = ports.db_by_ticker(&hit.ticker).await {
                return ResolveResult {
                    company: Some(from_db(complete)),
                    reason: None,
                };
            }
        }

        if let Some(us) = extract_us_ticker_token(q, &[]) {
            if let Some(company) = Self::verify_candidate(ports, &us, None, None).await {
                return ResolveResult {
                    company: Some(company),
                    reason: None,
                };
            }
        }

        if q.chars().any(|ch| ch.is_ascii_alphabetic()) {
            if let Some(hit) = ports.fmp_search_name(q).await.into_iter().find(|item| {
                item.exchange
                    .as_deref()
                    .is_some_and(echo_data::is_us_main_exchange)
            }) {
                if let Some(company) =
                    Self::verify_candidate(ports, &hit.symbol, None, Some(hit.name.as_str())).await
                {
                    return ResolveResult {
                        company: Some(company),
                        reason: None,
                    };
                }
            }
        }

        ResolveResult {
            company: None,
            reason: Some("not_found".into()),
        }
    }

    /// 研究链路入口：有 ticker 则库命中或外部验证；无 ticker 则对问题跑 resolve。
    /// 调用方在拿到 `Some` 后负责 `ensure` 建档。
    pub async fn resolve_research_company<P: CompanyResolvePorts>(
        ports: &P,
        ticker: Option<&str>,
        name_zh: Option<&str>,
        question: &str,
    ) -> Option<ResolvedListing> {
        if let Some(ticker) = ticker.map(str::trim).filter(|value| !value.is_empty()) {
            if let Some(existing) = ports.db_by_ticker(ticker).await {
                return Some(from_db(existing));
            }
            return Self::verify_candidate(ports, ticker, name_zh, None).await;
        }
        Self::resolve_query(ports, question).await.company
    }

    pub async fn verify_ticker<P: CompanyResolvePorts>(ports: &P, ticker: &str) -> VerifyResult {
        let symbol = ticker.trim();
        if symbol.is_empty() {
            return VerifyResult {
                status: VerifyStatus::NotFound,
                name: None,
                suggestions: Vec::new(),
            };
        }
        if let Some(existing) = ports.db_by_ticker(symbol).await {
            return VerifyResult {
                status: VerifyStatus::Verified,
                name: Some(
                    non_empty(&existing.name_zh)
                        .or_else(|| existing.name_en.clone())
                        .unwrap_or_else(|| existing.ticker.clone()),
                ),
                suggestions: Vec::new(),
            };
        }
        if looks_like_us(symbol) {
            if let Some(exact) = ports.fmp_exact_us(symbol).await {
                return VerifyResult {
                    status: VerifyStatus::Verified,
                    name: Some(if exact.name.is_empty() {
                        exact.symbol
                    } else {
                        exact.name
                    }),
                    suggestions: Vec::new(),
                };
            }
        }
        if ports.quote_alive(symbol).await {
            return VerifyResult {
                status: VerifyStatus::Verified,
                name: Some(String::new()),
                suggestions: Vec::new(),
            };
        }

        let mut suggestions = Vec::new();
        for hit in ports.db_search(symbol, 5).await {
            suggestions.push(ResolveSuggestion {
                ticker: hit.ticker.clone(),
                name: non_empty(&hit.name_zh)
                    .or(hit.name_en)
                    .unwrap_or(hit.ticker),
            });
        }
        for hit in ports.fmp_search_name(symbol).await {
            if hit
                .exchange
                .as_deref()
                .is_some_and(echo_data::is_us_main_exchange)
            {
                suggestions.push(ResolveSuggestion {
                    ticker: hit.symbol.clone(),
                    name: if hit.name.is_empty() {
                        hit.symbol
                    } else {
                        hit.name
                    },
                });
            }
        }
        suggestions.dedup_by(|a, b| a.ticker == b.ticker);
        suggestions.truncate(5);
        VerifyResult {
            status: VerifyStatus::NotFound,
            name: None,
            suggestions,
        }
    }

    async fn verify_candidate<P: CompanyResolvePorts>(
        ports: &P,
        ticker: &str,
        name_zh: Option<&str>,
        name_en: Option<&str>,
    ) -> Option<ResolvedListing> {
        let market = echo_data::detect_market(ticker);
        if market == echo_data::Market::Unsupported {
            return None;
        }
        if let Some(existing) = ports.db_by_ticker(ticker).await {
            return Some(from_db(existing));
        }
        if market == echo_data::Market::Us {
            if let Some(exact) = ports.fmp_exact_us(ticker).await {
                return Some(ResolvedListing {
                    ticker: exact.symbol.clone(),
                    name_zh: name_zh
                        .map(str::to_string)
                        .filter(|value| !value.is_empty())
                        .or_else(|| non_empty(&exact.name))
                        .unwrap_or_else(|| exact.symbol.clone()),
                    name_en: name_en
                        .map(str::to_string)
                        .filter(|value| !value.is_empty())
                        .or_else(|| non_empty(&exact.name))
                        .unwrap_or_default(),
                    industry: "美股".into(),
                    source: ResolveSource::Fmp,
                });
            }
        }
        if ports.quote_alive(ticker).await {
            let normalized = echo_data::normalize_ticker(ticker);
            return Some(ResolvedListing {
                ticker: normalized.clone(),
                name_zh: name_zh
                    .or(name_en)
                    .map(str::to_string)
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| normalized.clone()),
                name_en: name_en.unwrap_or("").to_string(),
                industry: if market == echo_data::Market::Hk {
                    "港股".into()
                } else {
                    "美股".into()
                },
                source: ResolveSource::Probe,
            });
        }
        None
    }
}

fn from_db(row: DbCompanyHit) -> ResolvedListing {
    ResolvedListing {
        ticker: row.ticker.clone(),
        name_zh: non_empty(&row.name_zh).unwrap_or_else(|| row.ticker.clone()),
        name_en: row.name_en.unwrap_or_default(),
        industry: row.industry.unwrap_or_default(),
        source: ResolveSource::Db,
    }
}

fn non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn looks_like_us(ticker: &str) -> bool {
    echo_data::detect_market(ticker) == echo_data::Market::Us
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    struct FakePorts {
        db: HashMap<String, DbCompanyHit>,
        search: Vec<DbCompanyHit>,
        fmp_exact: HashMap<String, ExternalSymbolHit>,
        fmp_name: Vec<ExternalSymbolHit>,
        alive: HashMap<String, bool>,
    }

    impl CompanyResolvePorts for FakePorts {
        async fn db_by_ticker(&self, ticker: &str) -> Option<DbCompanyHit> {
            self.db.get(&ticker.to_ascii_uppercase()).cloned()
        }

        async fn db_search(&self, _query: &str, limit: i64) -> Vec<DbCompanyHit> {
            self.search.iter().take(limit as usize).cloned().collect()
        }

        async fn fmp_exact_us(&self, ticker: &str) -> Option<ExternalSymbolHit> {
            self.fmp_exact.get(&ticker.to_ascii_uppercase()).cloned()
        }

        async fn fmp_search_name(&self, _name: &str) -> Vec<ExternalSymbolHit> {
            self.fmp_name.clone()
        }

        async fn quote_alive(&self, ticker: &str) -> bool {
            self.alive
                .get(&ticker.to_ascii_uppercase())
                .copied()
                .unwrap_or(false)
        }
    }

    #[tokio::test]
    async fn alias_then_probe_resolves_without_db() {
        let ports = FakePorts {
            alive: HashMap::from([("NVDA".into(), true)]),
            ..FakePorts::default()
        };
        let result = CompanyResolveService::resolve_query(&ports, "英伟达怎么样").await;
        let company = result.company.expect("resolved");
        assert_eq!(company.ticker, "NVDA");
        assert_eq!(company.source, ResolveSource::Alias);
    }

    #[tokio::test]
    async fn verify_returns_suggestions_when_unknown() {
        let ports = FakePorts {
            search: vec![DbCompanyHit {
                ticker: "AAPL".into(),
                name_zh: "苹果".into(),
                name_en: Some("Apple".into()),
                industry: None,
            }],
            fmp_name: vec![ExternalSymbolHit {
                symbol: "AMZN".into(),
                name: "Amazon".into(),
                exchange: Some("NASDAQ".into()),
            }],
            ..FakePorts::default()
        };
        let result = CompanyResolveService::verify_ticker(&ports, "ZZZZ").await;
        assert_eq!(result.status, VerifyStatus::NotFound);
        assert_eq!(result.suggestions.len(), 2);
    }

    #[tokio::test]
    async fn research_entry_verifies_explicit_ticker() {
        let ports = FakePorts {
            fmp_exact: HashMap::from([(
                "AAPL".into(),
                ExternalSymbolHit {
                    symbol: "AAPL".into(),
                    name: "Apple Inc.".into(),
                    exchange: Some("NASDAQ".into()),
                },
            )]),
            ..FakePorts::default()
        };
        let company = CompanyResolveService::resolve_research_company(
            &ports,
            Some("AAPL"),
            None,
            "估值怎么样",
        )
        .await
        .expect("verified");
        assert_eq!(company.ticker, "AAPL");
        assert_eq!(company.source, ResolveSource::Fmp);
    }

    #[tokio::test]
    async fn research_entry_falls_back_to_question_resolve() {
        let ports = FakePorts {
            alive: HashMap::from([("NVDA".into(), true)]),
            ..FakePorts::default()
        };
        let company =
            CompanyResolveService::resolve_research_company(&ports, None, None, "英伟达护城河？")
                .await
                .expect("resolved");
        assert_eq!(company.ticker, "NVDA");
    }
}
