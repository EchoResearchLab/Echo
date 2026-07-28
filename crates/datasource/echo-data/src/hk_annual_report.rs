//! 港股财务缺口的 HKEX 年报 PDF 补救路径。
//!
//! 只接受披露易官方 PDF，优先最近 FY 业绩公告。文本提取后采用保守标签匹配：金额单位、币种、
//! 收入与净利润至少要同时成立，再走 [`normalize_hk_financials`] 的单位和数量级质量门；任何
//! 一步不确定就只返回公告引用，不把猜测数字写入财务事实。

use crate::{
    HkexAnnouncement, HkexError, HkexService, NormalizedHkFinancials, RawHkFinancials,
    ingest_hk_financials,
};
use echo_db::Pool;
use regex::Regex;
use rust_decimal::Decimal;
use std::sync::LazyLock;
use std::time::Duration;

const MAX_PDF_BYTES: u64 = 30 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct HkAnnualReportRecovery {
    pub announcement: HkexAnnouncement,
    pub financials: Option<NormalizedHkFinancials>,
}

#[derive(Debug, thiserror::Error)]
pub enum HkAnnualReportError {
    #[error(transparent)]
    Hkex(#[from] HkexError),
    #[error("HKEX 最近三年没有 FY 业绩公告")]
    AnnualReportMissing,
    #[error("HKEX 年报 PDF 超过大小上限")]
    PdfTooLarge,
    #[error("HKEX 年报 PDF 文本提取失败：{0}")]
    PdfExtract(String),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Ingest(#[from] crate::HkFinancialsIngestError),
    #[error("HKEX 年报未通过严格字段匹配")]
    FinancialsNotParsed,
}

#[derive(Clone)]
pub struct HkAnnualReportService {
    client: reqwest::Client,
    pool: Pool,
    hkex: HkexService,
}

impl HkAnnualReportService {
    pub fn new(pool: Pool) -> Result<Self, HkAnnualReportError> {
        let client = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 EchoResearch/1.0 HKEX annual-report fallback")
            .timeout(Duration::from_secs(25))
            .build()?;
        Ok(Self {
            client,
            pool,
            hkex: HkexService::new()?,
        })
    }

    /// 找到 FY 公告后，即使 PDF 数字无法严格解析，也返回公告本身供研究链降级引用。
    pub async fn recover(
        &self,
        ticker: &str,
    ) -> Result<HkAnnualReportRecovery, HkAnnualReportError> {
        let announcement = self
            .hkex
            .results_announcements(ticker, 3, 30)
            .await?
            .into_iter()
            .find(|item| item.period_type.as_deref() == Some("FY"))
            .ok_or(HkAnnualReportError::AnnualReportMissing)?;

        let parsed = match self.download_and_parse(ticker, &announcement).await {
            Ok(raw) => match ingest_hk_financials(&self.pool, raw).await {
                Ok(normalized) => Some(normalized),
                Err(error) => {
                    tracing::warn!(
                        ticker,
                        error = %error,
                        "HKEX 年报已解析，但单位/数量级质量门拒绝入库"
                    );
                    None
                }
            },
            Err(error) => {
                tracing::warn!(
                    ticker,
                    error = %error,
                    "HKEX 年报已发现，但严格财务解析未通过"
                );
                None
            }
        };
        Ok(HkAnnualReportRecovery {
            announcement,
            financials: parsed,
        })
    }

    async fn download_and_parse(
        &self,
        ticker: &str,
        announcement: &HkexAnnouncement,
    ) -> Result<RawHkFinancials, HkAnnualReportError> {
        let response = self
            .client
            .get(&announcement.url)
            .send()
            .await?
            .error_for_status()?;
        if response
            .content_length()
            .is_some_and(|length| length > MAX_PDF_BYTES)
        {
            return Err(HkAnnualReportError::PdfTooLarge);
        }
        let bytes = response.bytes().await?;
        if bytes.len() as u64 > MAX_PDF_BYTES {
            return Err(HkAnnualReportError::PdfTooLarge);
        }
        let owned = bytes.to_vec();
        let text = tokio::task::spawn_blocking(move || pdf_extract::extract_text_from_mem(&owned))
            .await
            .map_err(|error| HkAnnualReportError::PdfExtract(error.to_string()))?
            .map_err(|error| HkAnnualReportError::PdfExtract(error.to_string()))?;
        parse_annual_report_text(ticker, announcement, &text)
            .ok_or(HkAnnualReportError::FinancialsNotParsed)
    }
}

static NUMBER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?x)(?:\(\s*)?-?\d[\d,]*(?:\.\d+)?(?:\s*\))?").expect("number regex")
});

#[must_use]
pub fn parse_annual_report_text(
    ticker: &str,
    announcement: &HkexAnnouncement,
    text: &str,
) -> Option<RawHkFinancials> {
    let (currency, source_unit) = detect_currency_unit(text)?;
    let lines = text
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();

    let (revenue, revenue_prior) = find_amount_pair(
        &lines,
        &[
            "收入",
            "收益",
            "營業收入",
            "营业收入",
            "revenue",
            "turnover",
        ],
    )?;
    let (net_income, net_income_prior) = find_amount_pair(
        &lines,
        &[
            "本公司權益持有人應佔盈利",
            "本公司权益持有人应占盈利",
            "股東應佔溢利",
            "股东应占溢利",
            "年度盈利",
            "年內盈利",
            "profit for the year",
            "profit attributable",
            "net profit",
        ],
    )?;

    let gross = find_amount_pair(&lines, &["毛利", "毛利潤", "毛利润", "gross profit"]);
    let operating = find_amount_pair(
        &lines,
        &[
            "經營盈利",
            "经营盈利",
            "營業利潤",
            "营业利润",
            "operating profit",
        ],
    );
    let operating_cash_flow = find_first_amount(
        &lines,
        &[
            "經營活動所得現金淨額",
            "经营活动所得现金净额",
            "經營活動產生的現金流量淨額",
            "经营活动产生的现金流量净额",
            "net cash generated from operating activities",
        ],
    );
    let cash = find_first_amount(
        &lines,
        &[
            "現金及現金等價物",
            "现金及现金等价物",
            "cash and cash equivalents",
        ],
    );
    let eps = find_eps(
        &lines,
        &[
            "每股基本盈利",
            "每股盈利－基本",
            "每股收益－基本",
            "basic earnings per share",
        ],
    );

    Some(RawHkFinancials {
        ticker: ticker.into(),
        period_label: announcement.period_label.clone(),
        period_end: announcement.period_end.clone(),
        period_type: announcement.period_type.clone(),
        currency: currency.into(),
        source_unit: source_unit.into(),
        revenue: Some(revenue),
        revenue_prior: Some(revenue_prior),
        gross_profit: gross.map(|values| values.0),
        gross_profit_prior: gross.map(|values| values.1),
        operating_income: operating.map(|values| values.0),
        operating_income_prior: operating.map(|values| values.1),
        net_income: Some(net_income),
        net_income_prior: Some(net_income_prior),
        net_income_attributable: Some(net_income),
        eps,
        operating_cash_flow,
        cash_and_equivalents: cash,
        net_cash: None,
        free_cash_flow: None,
        source_title: announcement.title.clone(),
        source_url: announcement.url.clone(),
        published_at: announcement.published_at,
    })
}

fn detect_currency_unit(text: &str) -> Option<(&'static str, &'static str)> {
    let currencies: [(&str, &[&str]); 3] = [
        ("CNY", &["人民幣", "人民币", "rmb", "renminbi"]),
        ("HKD", &["港幣", "港币", "港元", "hk$", "hkd"]),
        ("USD", &["美元", "us$", "usd", "u.s.dollar"]),
    ];
    let units = [
        ("十億元", ["十億元", "十亿元", "billion"]),
        ("百萬元", ["百萬元", "百万元", "million"]),
        ("千元", ["千元", "'000", "thousand"]),
    ];
    text.lines().find_map(|line| {
        let compact = line
            .to_ascii_lowercase()
            .replace([' ', '\r', '（', '）', '(', ')'], "");
        let declares_unit = [
            "除另有註明",
            "除另有注明",
            "金額單位",
            "金额单位",
            "amountsin",
            "expressedin",
            "unlessotherwisestated",
        ]
        .iter()
        .any(|cue| compact.contains(cue));
        if !declares_unit {
            return None;
        }
        let currency = currencies
            .iter()
            .find(|(_, aliases)| aliases.iter().any(|alias| compact.contains(alias)))?
            .0;
        let unit = units
            .iter()
            .find(|(_, aliases)| aliases.iter().any(|alias| compact.contains(alias)))?
            .0;
        Some((currency, unit))
    })
}

fn find_amount_pair(lines: &[String], aliases: &[&str]) -> Option<(Decimal, Decimal)> {
    let values = values_after_label(lines, aliases, true)?;
    Some((*values.first()?, *values.get(1)?))
}

fn find_first_amount(lines: &[String], aliases: &[&str]) -> Option<Decimal> {
    values_after_label(lines, aliases, true)?.first().copied()
}

fn find_eps(lines: &[String], aliases: &[&str]) -> Option<Decimal> {
    values_after_label(lines, aliases, false)?.first().copied()
}

fn values_after_label(
    lines: &[String],
    aliases: &[&str],
    prefer_large: bool,
) -> Option<Vec<Decimal>> {
    for (index, line) in lines.iter().enumerate() {
        let lower = line.to_ascii_lowercase();
        if !aliases
            .iter()
            .any(|alias| lower.contains(&alias.to_ascii_lowercase()))
        {
            continue;
        }
        let window = lines[index..lines.len().min(index + 9)].join(" ");
        let mut values = NUMBER_RE
            .find_iter(&window)
            .filter_map(|matched| parse_number(matched.as_str()))
            .filter(|value| {
                let integer = value.trunc();
                !(integer == *value
                    && *value >= Decimal::from(1_900)
                    && *value <= Decimal::from(2_100))
            })
            .collect::<Vec<_>>();
        if prefer_large {
            let large = values
                .iter()
                .copied()
                .filter(|value| value.abs() >= Decimal::from(100))
                .collect::<Vec<_>>();
            if large.len() >= 2 {
                values = large;
            }
        }
        if !values.is_empty() {
            return Some(values);
        }
    }
    None
}

fn parse_number(input: &str) -> Option<Decimal> {
    let negative_parentheses = input.trim().starts_with('(') && input.trim().ends_with(')');
    let cleaned = input
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .replace([',', ' '], "");
    let value: Decimal = cleaned.parse().ok()?;
    Some(if negative_parentheses {
        -value.abs()
    } else {
        value
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use rust_decimal_macros::dec;

    fn announcement() -> HkexAnnouncement {
        HkexAnnouncement {
            title: "截至2025年12月31日止年度業績公告".into(),
            filing_type: "年度業績".into(),
            news_id: "x".into(),
            published_at: Utc.with_ymd_and_hms(2026, 3, 18, 8, 0, 0).single(),
            url: "https://www1.hkexnews.hk/listedco/listconews/sehk/2026/a.pdf".into(),
            period_end: Some("2025-12-31".into()),
            period_type: Some("FY".into()),
            period_label: Some("2025 FY".into()),
        }
    }

    #[test]
    fn strict_text_parser_extracts_unit_and_core_rows() {
        let text = r#"
            （除另有註明外，金額單位為人民幣百萬元）
            收入
            751,766 660,257
            毛利
            427,366 354,100
            本公司權益持有人應佔盈利
            228,011 194,073
            每股基本盈利（人民幣元）
            24.56 20.91
            經營活動所得現金淨額
            310,000 280,000
        "#;
        let raw = parse_annual_report_text("0700.HK", &announcement(), text).expect("parsed");
        assert_eq!(raw.currency, "CNY");
        assert_eq!(raw.source_unit, "百萬元");
        assert_eq!(raw.revenue, Some(dec!(751766)));
        assert_eq!(raw.revenue_prior, Some(dec!(660257)));
        assert_eq!(raw.net_income, Some(dec!(228011)));
        assert_eq!(raw.eps, Some(dec!(24.56)));
    }

    #[test]
    fn refuses_pdf_text_without_explicit_unit() {
        let text = "收入 751,766 660,257\n年度盈利 228,011 194,073";
        assert!(parse_annual_report_text("0700.HK", &announcement(), text).is_none());
    }
}
