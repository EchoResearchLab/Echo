//! 自然语言问句里的 ticker 实体抽取——纯规则，不做供应商验证。
//!
//! 数字尤其危险：成本价、股数、估值阈值绝不能静默变成港股代码。
//! 所有抽取入口先走 [`normalize_question_text`]。

use fancy_regex::Regex;
use std::collections::HashSet;
use std::sync::LazyLock;

static MONEY_OR_QUANTITY_SUFFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*(?:块钱?|元|美元|美金|港元|港币|人民币|股|手|万|亿|%|％|倍|年|个月|月|天|日|个基点|基点|个百分点|个点|bp)")
        .expect("suffix regex")
});

static MONEY_OR_QUANTITY_PREFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?:成本价?|买入价?|购入价?|入仓价?|现价|价格|目标价|止损价?|止盈价?|市值|持有|买了|买的|买入|购入|入手|跌到|涨到|回撤到)\s*[^\d]{0,4}$",
    )
    .expect("prefix regex")
});

static COMMON_NON_TICKERS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    HashSet::from([
        "PE", "PB", "PS", "ROE", "ROI", "ROA", "ROC", "AI", "IPO", "GDP", "CEO", "CFO", "COO",
        "CTO", "CMO", "US", "HK", "EPS", "FCF", "DCF", "ETF", "ADR", "Q1", "Q2", "Q3", "Q4", "YOY",
        "QOQ", "MOM", "TTM", "LTM", "MRQ", "CPI", "PPI", "PMI", "GNP", "EV", "NAV", "AUM", "BPS",
        "DPS", "NIM", "NYSE", "SEC", "SFC", "MSCI", "FTSE", "ESG", "SPAC", "SPX", "SPY", "QQQ",
        "DIA", "IWM", "VOO", "VTI", "VT", "IVV", "ARKK", "HSI", "EEM", "FXI", "KWEB", "WHAT",
        "ABOUT", "HOW", "THE", "FOR", "AND", "BUY", "SELL", "PRICE", "STOCK",
    ])
});

/// 全角 → 半角，并清掉零宽字符。
#[must_use]
pub fn normalize_question_text(text: &str) -> String {
    let half: String = text
        .chars()
        .map(|ch| {
            let code = ch as u32;
            if (0xFF01..=0xFF5E).contains(&code) {
                char::from_u32(code - 0xFEE0).unwrap_or(ch)
            } else if ch == '\u{3000}' {
                ' '
            } else {
                ch
            }
        })
        .collect();
    half.chars()
        .filter(|ch| {
            !matches!(
                *ch,
                '\u{200B}'
                    | '\u{200C}'
                    | '\u{200D}'
                    | '\u{200E}'
                    | '\u{200F}'
                    | '\u{2060}'
                    | '\u{FEFF}'
            )
        })
        .collect()
}

fn normalize_hk_digits(digits: &str) -> String {
    format!("{digits:0>4}.HK")
}

/// 抽取港股代码；价格/数量语境中的数字一律拒绝。
#[must_use]
pub fn extract_hk_ticker(text: &str) -> Option<String> {
    let raw = normalize_question_text(text);

    static EXPLICIT_SUFFIX: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)(?:^|[^\dA-Za-z])(\d{1,5})\s*(?:\.\s*)?HK(?![A-Za-z])").expect("hk suffix")
    });
    if let Ok(Some(caps)) = EXPLICIT_SUFFIX.captures(&raw) {
        return Some(normalize_hk_digits(caps.get(1)?.as_str()));
    }

    static EXPLICIT_PREFIX: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)(?:港股|股票代码|证券代码|代码)\s*[:：]?\s*(\d{1,5})(?!\d)")
            .expect("hk prefix")
    });
    if let Ok(Some(caps)) = EXPLICIT_PREFIX.captures(&raw) {
        return Some(normalize_hk_digits(caps.get(1)?.as_str()));
    }

    static ONLY_NUMBER: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^(\d{1,5})$").expect("only number"));
    if let Ok(Some(caps)) = ONLY_NUMBER.captures(raw.trim()) {
        return Some(normalize_hk_digits(caps.get(1)?.as_str()));
    }

    static IMPLICIT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?<![\d.])(\d{3,5})(?![\d.])").expect("implicit hk"));
    for caps in IMPLICIT.captures_iter(&raw).flatten() {
        let Some(matched) = caps.get(0) else { continue };
        let Some(digits) = caps.get(1) else { continue };
        let start = matched.start();
        let end = matched.end();
        let before = slice_back(&raw, start, 12);
        let after = slice_forward(&raw, end, 12);
        if MONEY_OR_QUANTITY_PREFIX.is_match(before).unwrap_or(false)
            || MONEY_OR_QUANTITY_SUFFIX.is_match(after).unwrap_or(false)
        {
            continue;
        }
        return Some(normalize_hk_digits(digits.as_str()));
    }
    None
}

fn slice_back(text: &str, end: usize, max_bytes: usize) -> &str {
    let mut start = end.saturating_sub(max_bytes);
    while start > 0 && !text.is_char_boundary(start) {
        start -= 1;
    }
    &text[start..end]
}

fn slice_forward(text: &str, start: usize, max_bytes: usize) -> &str {
    let mut end = (start + max_bytes).min(text.len());
    while end > start && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[start..end]
}

/// 停用词拷贝，供前端/解析层共用。
#[must_use]
pub fn common_non_tickers() -> HashSet<&'static str> {
    COMMON_NON_TICKERS.clone()
}

/// 抽取美股代码词元；结果仍需供应商验证后才能研究。
#[must_use]
pub fn extract_us_ticker_token(text: &str, additional_stopwords: &[&str]) -> Option<String> {
    let raw = normalize_question_text(text);
    let raw = raw.trim();
    let mut stopwords: HashSet<String> = COMMON_NON_TICKERS
        .iter()
        .map(|word| (*word).to_string())
        .collect();
    for word in additional_stopwords {
        stopwords.insert(word.to_ascii_uppercase());
    }

    static DOLLAR: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\$([A-Za-z][A-Za-z.-]{0,6})\b").expect("dollar"));
    static DOT_US: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)\b([A-Za-z][A-Za-z.-]{0,6})\.US\b").expect("dot us"));
    let explicit = DOLLAR
        .captures(raw)
        .ok()
        .flatten()
        .or_else(|| DOT_US.captures(raw).ok().flatten());
    if let Some(caps) = explicit {
        let ticker = caps.get(1)?.as_str().to_ascii_uppercase();
        return (!stopwords.contains(&ticker)).then_some(ticker);
    }

    static BARE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^[A-Za-z][A-Za-z.-]{0,6}$").expect("bare"));
    if BARE.is_match(raw).unwrap_or(false) {
        let ticker = raw.to_ascii_uppercase();
        return (!stopwords.contains(&ticker)).then_some(ticker);
    }

    let has_cjk = raw
        .chars()
        .any(|ch| ('\u{3400}'..='\u{9fff}').contains(&ch));
    static TOKEN: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"[A-Za-z][A-Za-z.-]{1,6}").expect("token"));
    let mut candidates = Vec::new();
    for caps in TOKEN.captures_iter(raw).flatten() {
        let Some(matched) = caps.get(0) else { continue };
        let token = matched.as_str();
        if token.len() > 5 {
            continue;
        }
        let after = &raw[matched.end()..];
        if after.starts_with(|ch: char| ch.is_whitespace())
            && after
                .trim_start()
                .starts_with(|ch: char| ch.is_ascii_alphabetic())
        {
            // “OPEN AI / SPACE X” —— 不把第一个词误作裸 ticker。
            continue;
        }
        let upper = token.to_ascii_uppercase();
        let lower = token.to_ascii_lowercase();
        let ok_case = token == upper || (token == lower.as_str() && (has_cjk || token.len() >= 4));
        if ok_case && !stopwords.contains(&upper) {
            candidates.push(upper);
        }
    }
    candidates.pop()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn investor_questions_match_baseline_matrix() {
        let cases = [
            ("86块钱的rklb怎么样", None, Some("RKLB")),
            ("我 86 美元买的 RKLB 现在怎么看", None, Some("RKLB")),
            ("成本 700 元的腾讯怎么办", None, None),
            ("持有 700 股腾讯，风险大吗", None, None),
            ("PE 小于 40 的公司有哪些", None, None),
            ("分析一下 1316.HK", Some("1316.HK"), None),
            ("港股 700 怎么样", Some("0700.HK"), None),
            ("700怎么样", Some("0700.HK"), None),
            ("$rklb 的现金流如何", None, Some("RKLB")),
            ("rklb", None, Some("RKLB")),
            ("what about rklb", None, Some("RKLB")),
            ("OPEN AI 上市了吗", None, None),
            ("Rocket Lab怎么样", None, None),
        ];
        for (question, hk, us) in cases {
            assert_eq!(
                extract_hk_ticker(question).as_deref(),
                hk,
                "HK mismatch: {question}"
            );
            assert_eq!(
                extract_us_ticker_token(question, &[]).as_deref(),
                us,
                "US mismatch: {question}"
            );
        }
    }
}
