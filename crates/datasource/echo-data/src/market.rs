#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Market {
    Us,
    Hk,
    Unsupported,
}

#[must_use]
pub fn detect_market(ticker: &str) -> Market {
    let value = ticker.trim().to_ascii_uppercase();
    let bare_digits = !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit());
    let a_share = value
        .strip_suffix(".SS")
        .or_else(|| value.strip_suffix(".SZ"))
        .is_some_and(|code| code.len() == 6 && code.bytes().all(|byte| byte.is_ascii_digit()));
    if a_share || (bare_digits && value.len() == 6) {
        Market::Unsupported
    } else if bare_digits && value.len() <= 5
        || value.strip_suffix(".HK").is_some_and(|code| {
            !code.is_empty() && code.len() <= 5 && code.bytes().all(|b| b.is_ascii_digit())
        })
    {
        Market::Hk
    } else {
        Market::Us
    }
}

#[must_use]
pub fn normalize_ticker(ticker: &str) -> String {
    let value = ticker.trim().to_ascii_uppercase();
    if detect_market(&value) != Market::Hk {
        return value;
    }
    let code = value.strip_suffix(".HK").unwrap_or(&value);
    format!("{code:0>4}.HK")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn market_detection_keeps_retired_a_shares_unsupported() {
        assert_eq!(detect_market("600519.SS"), Market::Unsupported);
        assert_eq!(detect_market("000001"), Market::Unsupported);
        assert_eq!(detect_market("700"), Market::Hk);
        assert_eq!(detect_market("0700.HK"), Market::Hk);
        assert_eq!(detect_market("AAPL"), Market::Us);
        assert_eq!(normalize_ticker("700"), "0700.HK");
    }
}
