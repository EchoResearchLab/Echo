//! HKEX 披露易业绩公告发现。
//!
//! 本层只负责“股票代码 → 官方业绩公告 PDF 清单 + 报告期间”，不解析 PDF 数字、不写财务表。
//! 下游提取器必须把原始金额和明确单位交给 `normalize_hk_financials`，才能获得绝对值写入资格。

use crate::{Market, detect_market, normalize_ticker};
use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, NaiveDateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

const HKEX_BASE: &str = "https://www1.hkexnews.hk";
const NOISE_TITLES: &[&str] = &[
    "澄清",
    "更正",
    "補充",
    "补充",
    "取消",
    "延遲",
    "延迟",
    "翌日",
    "議程",
    "议程",
    "通函",
    "代表委任",
];

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct HkexAnnouncement {
    pub title: String,
    pub filing_type: String,
    pub news_id: String,
    pub published_at: Option<DateTime<Utc>>,
    pub url: String,
    pub period_end: Option<String>,
    pub period_type: Option<String>,
    pub period_label: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HkexPeriod {
    pub period_end: String,
    pub period_type: String,
    pub period_label: String,
}

#[derive(Debug, thiserror::Error)]
pub enum HkexError {
    #[error("港交所公告发现只支持港股代码：{0}")]
    UnsupportedTicker(String),
    #[error("HKEX 披露易没有匹配到 {0} 的 stockId")]
    StockIdMissing(String),
    #[error("HKEX 响应格式无效：{0}")]
    InvalidResponse(String),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
}

#[derive(Clone)]
pub struct HkexService {
    client: reqwest::Client,
}

impl HkexService {
    pub fn new() -> Result<Self, HkexError> {
        let client = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 EchoResearch/1.0 HKEX filings pipeline")
            .timeout(Duration::from_secs(20))
            .build()?;
        Ok(Self { client })
    }

    /// 搜索最近若干年的官方业绩公告，按发布时间新→旧返回。
    pub async fn results_announcements(
        &self,
        raw_ticker: &str,
        years_back: i32,
        limit: usize,
    ) -> Result<Vec<HkexAnnouncement>, HkexError> {
        let ticker = normalize_ticker(raw_ticker);
        if detect_market(&ticker) != Market::Hk {
            return Err(HkexError::UnsupportedTicker(ticker));
        }
        let stock_id = self.lookup_stock_id(&ticker).await?;
        let now = Utc::now();
        let from = format!("{}0101", now.year().saturating_sub(years_back.max(1)));
        let to = now.format("%Y%m%d").to_string();
        let response = self
            .client
            .get(format!("{HKEX_BASE}/search/titleSearchServlet.do"))
            .query(&[
                ("sortDir", "0".to_string()),
                ("sortByOptions", "DateTime".to_string()),
                ("category", "0".to_string()),
                ("market", "SEHK".to_string()),
                ("stockId", stock_id.to_string()),
                ("documentType", "-1".to_string()),
                ("fromDate", from),
                ("toDate", to),
                ("title", "業績".to_string()),
                ("searchType", "1".to_string()),
                ("t1code", "-2".to_string()),
                ("t2Gcode", "-2".to_string()),
                ("t2code", "-2".to_string()),
                ("rowRange", limit.clamp(1, 100).to_string()),
                ("lang", "zh".to_string()),
            ])
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        let value: Value = serde_json::from_str(&response)
            .map_err(|error| HkexError::InvalidResponse(error.to_string()))?;
        Ok(parse_search_result(&value)
            .into_iter()
            .take(limit)
            .collect())
    }

    async fn lookup_stock_id(&self, ticker: &str) -> Result<i64, HkexError> {
        let code = ticker.trim_end_matches(".HK");
        let response = self
            .client
            .get(format!("{HKEX_BASE}/search/prefix.do"))
            .query(&[
                ("callback", "callback"),
                ("lang", "ZH"),
                ("type", "A"),
                ("name", code),
                ("market", "SEHK"),
            ])
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        parse_stock_id_jsonp(&response, code)
            .ok_or_else(|| HkexError::StockIdMissing(ticker.to_string()))
    }
}

#[derive(Debug, Deserialize)]
struct PrefixResponse {
    #[serde(rename = "stockInfo", default)]
    stock_info: Vec<PrefixStock>,
}

#[derive(Debug, Deserialize)]
struct PrefixStock {
    #[serde(rename = "stockId")]
    stock_id: i64,
    code: String,
}

fn parse_stock_id_jsonp(response: &str, requested_code: &str) -> Option<i64> {
    let start = response.find('(')? + 1;
    let end = response.rfind(')')?;
    let parsed: PrefixResponse = serde_json::from_str(response.get(start..end)?).ok()?;
    let requested = requested_code.parse::<u32>().ok()?;
    parsed
        .stock_info
        .into_iter()
        .find(|row| row.code.parse::<u32>().ok() == Some(requested))
        .map(|row| row.stock_id)
}

#[derive(Debug, Deserialize)]
struct SearchRow {
    #[serde(rename = "TITLE", default)]
    title: String,
    #[serde(rename = "LONG_TEXT", default)]
    long_text: String,
    #[serde(rename = "SHORT_TEXT", default)]
    short_text: String,
    #[serde(rename = "FILE_TYPE", default)]
    file_type: String,
    #[serde(rename = "NEWS_ID", default)]
    news_id: String,
    #[serde(rename = "DATE_TIME", default)]
    date_time: String,
    #[serde(rename = "FILE_LINK", default)]
    file_link: String,
}

fn parse_search_result(value: &Value) -> Vec<HkexAnnouncement> {
    let rows_value = match value.get("result") {
        Some(Value::String(encoded)) => serde_json::from_str(encoded).unwrap_or(Value::Null),
        Some(other) => other.clone(),
        None => Value::Null,
    };
    let rows: Vec<SearchRow> = serde_json::from_value(rows_value).unwrap_or_default();
    let mut announcements = rows
        .into_iter()
        .filter(|row| row.file_type.eq_ignore_ascii_case("PDF"))
        .filter_map(|row| {
            let title = decode_html(&row.title);
            let is_result = title.contains("業績")
                || title.contains("业绩")
                || title.to_ascii_uppercase().contains("RESULTS ANNOUNCEMENT");
            if !is_result || NOISE_TITLES.iter().any(|noise| title.contains(noise)) {
                return None;
            }
            let url = if row.file_link.starts_with("https://") {
                row.file_link
            } else if row.file_link.starts_with('/') {
                format!("{HKEX_BASE}{}", row.file_link)
            } else {
                return None;
            };
            let period = parse_period_from_title(&title);
            Some(HkexAnnouncement {
                title,
                filing_type: decode_html(if row.long_text.is_empty() {
                    &row.short_text
                } else {
                    &row.long_text
                }),
                news_id: row.news_id,
                published_at: parse_hkex_datetime(&row.date_time),
                url,
                period_end: period.as_ref().map(|value| value.period_end.clone()),
                period_type: period.as_ref().map(|value| value.period_type.clone()),
                period_label: period.map(|value| value.period_label),
            })
        })
        .collect::<Vec<_>>();
    announcements.sort_by(|a, b| b.published_at.cmp(&a.published_at));
    announcements
}

fn decode_html(input: &str) -> String {
    let mut output = input
        .replace("<br/>", " ")
        .replace("<br>", " ")
        .replace("&amp;", "&")
        .replace("&#x2f;", "/")
        .replace("&#x2F;", "/")
        .replace("&#39;", "'")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">");
    while let Some(start) = output.find('<') {
        let Some(relative_end) = output[start..].find('>') else {
            break;
        };
        output.replace_range(start..=start + relative_end, " ");
    }
    output.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn parse_hkex_datetime(input: &str) -> Option<DateTime<Utc>> {
    let local = NaiveDateTime::parse_from_str(input.trim(), "%d/%m/%Y %H:%M").ok()?;
    let hk = FixedOffset::east_opt(8 * 60 * 60)?;
    hk.from_local_datetime(&local)
        .single()
        .map(|date| date.with_timezone(&Utc))
}

/// 公告标题解析为报告期。覆盖“截至二零二六年三月三十一日…”与阿拉伯数字版本。
#[must_use]
pub fn parse_period_from_title(title: &str) -> Option<HkexPeriod> {
    let (year, month, day) = parse_title_date(title)?;
    let end = NaiveDate::from_ymd_opt(year, month, day)?;
    let quarter = ((month.saturating_sub(1)) / 3) + 1;
    let period_type = if title.contains("三個月")
        || title.contains("三个月")
        || title.contains("季度")
        || title.contains("第") && (title.contains("季度") || title.contains("季"))
    {
        format!("Q{quarter}")
    } else if title.contains("六個月") || title.contains("六个月") {
        "H1".to_string()
    } else if title.contains("九個月") || title.contains("九个月") {
        "9M".to_string()
    } else if title.contains("年度")
        || title.contains("全年")
        || title.contains("年業績")
        || title.contains("年业绩")
    {
        "FY".to_string()
    } else {
        return None;
    };
    let period_label = format!("{year} {period_type}");
    Some(HkexPeriod {
        period_end: end.format("%Y-%m-%d").to_string(),
        period_type,
        period_label,
    })
}

fn parse_title_date(title: &str) -> Option<(i32, u32, u32)> {
    if let Some(after) = title.split_once("截至").map(|(_, after)| after) {
        let (year_text, after_year) = after.split_once('年')?;
        let (month_text, after_month) = after_year.split_once('月')?;
        let (day_text, _) = after_month.split_once('日')?;
        return Some((
            parse_number(year_text.trim())? as i32,
            parse_number(month_text.trim())?,
            parse_number(day_text.trim())?,
        ));
    }

    // 阿里式“2026年三月底止季度”。
    let (year, after_year) = ascii_year_and_tail(title)?;
    if let Some((month_text, _)) = after_year.split_once("月底") {
        let month = parse_number(month_text.trim())?;
        return Some((year, month, last_day_of_month(year, month)?));
    }

    // 小鹏式“2025年第四季度及2025財政年度…”。
    if let Some(after_quarter) = after_year.split_once('第').map(|(_, tail)| tail) {
        let (quarter_text, _) = after_quarter.split_once("季度")?;
        let quarter = parse_number(quarter_text.trim())?;
        if (1..=4).contains(&quarter) {
            let month = quarter * 3;
            return Some((year, month, last_day_of_month(year, month)?));
        }
    }

    // 汇丰式“2025年業績”裸年度标题。
    if after_year.trim_start().starts_with("業績") || after_year.trim_start().starts_with("业绩")
    {
        return Some((year, 12, 31));
    }
    None
}

fn ascii_year_and_tail(title: &str) -> Option<(i32, &str)> {
    let bytes = title.as_bytes();
    for index in 0..=bytes.len().saturating_sub(4) {
        let candidate = &bytes[index..index + 4];
        if candidate.iter().all(|byte| byte.is_ascii_digit())
            && title.is_char_boundary(index)
            && title.is_char_boundary(index + 4)
        {
            let tail = &title[index + 4..];
            if let Some(after_year) = tail.strip_prefix('年') {
                let year = std::str::from_utf8(candidate).ok()?.parse().ok()?;
                return Some((year, after_year));
            }
        }
    }
    None
}

fn last_day_of_month(year: i32, month: u32) -> Option<u32> {
    let (next_year, next_month) = if month == 12 {
        (year.checked_add(1)?, 1)
    } else {
        (year, month.checked_add(1)?)
    };
    NaiveDate::from_ymd_opt(next_year, next_month, 1)?
        .pred_opt()
        .map(|date| date.day())
}

fn parse_number(input: &str) -> Option<u32> {
    if input.bytes().all(|byte| byte.is_ascii_digit()) {
        return input.parse().ok();
    }
    let digits = input
        .chars()
        .map(chinese_digit)
        .collect::<Option<Vec<_>>>()?;
    if input.contains('十') {
        let ten = input.find('十')?;
        let left = input[..ten]
            .chars()
            .next()
            .and_then(chinese_digit)
            .unwrap_or(1);
        let right = input[ten + '十'.len_utf8()..]
            .chars()
            .next()
            .and_then(chinese_digit)
            .unwrap_or(0);
        return Some(left * 10 + right);
    }
    digits.into_iter().try_fold(0_u32, |value, digit| {
        value.checked_mul(10)?.checked_add(digit)
    })
}

fn chinese_digit(value: char) -> Option<u32> {
    match value {
        '零' | '〇' => Some(0),
        '一' => Some(1),
        '二' => Some(2),
        '三' => Some(3),
        '四' => Some(4),
        '五' => Some(5),
        '六' => Some(6),
        '七' => Some(7),
        '八' => Some(8),
        '九' => Some(9),
        '十' => Some(10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jsonp_stock_lookup_matches_numeric_code_not_first_row() {
        let jsonp = r#"callback({"stockInfo":[{"stockId":1,"code":"80700"},{"stockId":7609,"code":"00700"}]});"#;
        assert_eq!(parse_stock_id_jsonp(jsonp, "0700"), Some(7609));
    }

    #[test]
    fn current_hkex_search_shape_parses_and_filters_noise() {
        let rows = serde_json::json!([
            {
                "TITLE": "截至二零二六年三月三十一日止三個月業績公佈",
                "LONG_TEXT": "公告及通告 - [季度業績]",
                "FILE_TYPE": "PDF",
                "NEWS_ID": "12157227",
                "DATE_TIME": "13/05/2026 16:31",
                "FILE_LINK": "/listedco/listconews/sehk/2026/0513/a_c.pdf"
            },
            {
                "TITLE": "補充公告－業績",
                "FILE_TYPE": "PDF",
                "FILE_LINK": "/noise.pdf"
            }
        ]);
        let response = serde_json::json!({ "result": rows.to_string() });
        let parsed = parse_search_result(&response);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].period_type.as_deref(), Some("Q1"));
        assert_eq!(parsed[0].period_end.as_deref(), Some("2026-03-31"));
        assert!(parsed[0].url.starts_with(HKEX_BASE));
        assert_eq!(
            parsed[0]
                .published_at
                .expect("published")
                .format("%Y-%m-%dT%H:%MZ")
                .to_string(),
            "2026-05-13T08:31Z"
        );
    }

    #[test]
    fn period_parser_handles_fy_and_multi_month_titles() {
        let fy =
            parse_period_from_title("截至二零二五年十二月三十一日止年度全年業績公佈").expect("fy");
        assert_eq!(fy.period_type, "FY");
        assert_eq!(fy.period_label, "2025 FY");
        let q2 = parse_period_from_title("截至二零二五年六月三十日止三個月及六個月業績公佈")
            .expect("q2");
        assert_eq!(q2.period_type, "Q2");
        assert_eq!(
            parse_period_from_title("2026年三月底止季度業績")
                .expect("month end")
                .period_end,
            "2026-03-31"
        );
        assert_eq!(
            parse_period_from_title("2025年第四季度及2025財政年度業績")
                .expect("quarter")
                .period_type,
            "Q4"
        );
        assert_eq!(
            parse_period_from_title("2025年業績")
                .expect("bare year")
                .period_type,
            "FY"
        );
    }
}
