use crate::{ProviderStatus, Quote};
use rust_decimal::Decimal;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Warn,
    Reject,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QualityIssue {
    pub field: &'static str,
    pub severity: Severity,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QualityReport {
    pub ok: bool,
    pub score: u8,
    pub issues: Vec<QualityIssue>,
}

#[must_use]
pub fn check_quote(quote: &Quote, now: chrono::DateTime<chrono::Utc>) -> QualityReport {
    if quote.status == ProviderStatus::Missing {
        return QualityReport {
            ok: true,
            score: 100,
            issues: Vec::new(),
        };
    }
    let mut issues = Vec::new();
    if quote.price.is_none_or(|price| price <= Decimal::ZERO) {
        issues.push(QualityIssue {
            field: "price",
            severity: Severity::Reject,
            message: "价格缺失或不是正数".into(),
        });
    }
    if quote.currency.as_deref().is_none_or(str::is_empty) {
        issues.push(QualityIssue {
            field: "currency",
            severity: Severity::Reject,
            message: "裸数字没有币种".into(),
        });
    }
    if now.signed_duration_since(quote.as_of).num_days() > 7 {
        issues.push(QualityIssue {
            field: "asOf",
            severity: Severity::Warn,
            message: "行情超过 7 天".into(),
        });
    }
    if quote
        .change_percent
        .is_some_and(|value| value.abs() > Decimal::from(40))
    {
        issues.push(QualityIssue {
            field: "changePercent",
            severity: Severity::Warn,
            message: "单日涨跌超过 40%，入库前需要复核".into(),
        });
    }
    let rejects = issues
        .iter()
        .filter(|issue| issue.severity == Severity::Reject)
        .count() as u8;
    let warnings = issues
        .iter()
        .filter(|issue| issue.severity == Severity::Warn)
        .count() as u8;
    QualityReport {
        ok: rejects == 0,
        score: 100u8
            .saturating_sub(rejects.saturating_mul(50))
            .saturating_sub(warnings.saturating_mul(15)),
        issues,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use rust_decimal_macros::dec;

    #[test]
    fn missing_is_honest_but_invalid_ok_quote_is_rejected() {
        let now = chrono::Utc.with_ymd_and_hms(2026, 7, 21, 0, 0, 0).unwrap();
        let missing = Quote::missing("AAPL", "test", now);
        assert!(check_quote(&missing, now).ok);
        let bad = Quote {
            status: ProviderStatus::Ok,
            price: Some(dec!(0)),
            ..missing
        };
        assert!(!check_quote(&bad, now).ok);
    }
}
