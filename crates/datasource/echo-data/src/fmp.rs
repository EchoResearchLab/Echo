//! FMP `stable` HTTP 公共件：鉴权查询、200+错误体探测、Decimal 解析。
//!
//! 免费档实测：退役 endpoint / premium 门槛常返回 HTTP 200 + `{ "Error Message": ... }`，
//! 不能只看 `response.ok`。

use rust_decimal::Decimal;
use serde_json::Value;
use std::time::Duration;

pub(crate) const FMP_STABLE_BASE: &str = "https://financialmodelingprep.com/stable";

#[derive(Debug, thiserror::Error)]
pub enum FmpError {
    #[error("FMP_API_KEY 未配置")]
    MissingApiKey,
    #[error("商用模式不允许未授权的 FMP 免费源")]
    CommercialBlocked,
    #[error("FMP 仅覆盖美股三表：{0}")]
    UnsupportedMarket(String),
    #[error("FMP {path}: {detail}")]
    Api { path: String, detail: String },
    #[error(transparent)]
    Http(#[from] reqwest::Error),
}

pub(crate) fn build_client() -> Result<reqwest::Client, FmpError> {
    Ok(reqwest::Client::builder()
        .user_agent("EchoResearch/1.0")
        .timeout(Duration::from_secs(8))
        .build()?)
}

pub(crate) async fn fetch_json(
    client: &reqwest::Client,
    api_key: &str,
    path: &str,
) -> Result<Value, FmpError> {
    let sep = if path.contains('?') { '&' } else { '?' };
    let url = format!("{FMP_STABLE_BASE}/{path}{sep}apikey={api_key}");
    let response = client.get(url).send().await?;
    let status = response.status();
    let body: Value = response.json().await?;
    if !status.is_success() {
        return Err(FmpError::Api {
            path: path.split('?').next().unwrap_or(path).into(),
            detail: format!("HTTP {status}"),
        });
    }
    if let Some(detail) = fmp_error_message(&body) {
        return Err(FmpError::Api {
            path: path.split('?').next().unwrap_or(path).into(),
            detail,
        });
    }
    Ok(body)
}

pub(crate) fn fmp_error_message(body: &Value) -> Option<String> {
    if body.is_array() {
        return None;
    }
    body.get("Error Message")
        .or_else(|| body.get("error"))
        .map(|value| match value {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        })
}

pub(crate) fn decimal(value: &Value) -> Option<Decimal> {
    match value {
        Value::Number(number) => number.to_string().parse().ok(),
        Value::String(text) => text.parse().ok(),
        _ => None,
    }
}

pub(crate) fn decimal_at(body: &Value, key: &str) -> Option<Decimal> {
    body.get(key).and_then(decimal)
}

pub(crate) fn string_at(body: &Value, key: &str) -> Option<String> {
    body.get(key).and_then(|value| match value {
        Value::String(text) if !text.is_empty() => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    })
}
