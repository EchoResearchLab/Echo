//! Deterministic financial arithmetic for Echo Research.
//!
//! This crate deliberately accepts and returns decimal values. Binary floating
//! point belongs at the display boundary, never in money, ratios or valuation.

use rust_decimal::{Decimal, RoundingStrategy};
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Currency {
    Cny,
    Hkd,
    Usd,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Money {
    amount: Decimal,
    currency: Currency,
}

impl Money {
    #[must_use]
    pub const fn new(amount: Decimal, currency: Currency) -> Self {
        Self { amount, currency }
    }

    #[must_use]
    pub const fn amount(self) -> Decimal {
        self.amount
    }

    #[must_use]
    pub const fn currency(self) -> Currency {
        self.currency
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FinanceError {
    CurrencyMismatch,
    NonPositiveShares,
    ArithmeticOverflow,
}

impl fmt::Display for FinanceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CurrencyMismatch => formatter.write_str("currency mismatch"),
            Self::NonPositiveShares => formatter.write_str("shares must be positive"),
            Self::ArithmeticOverflow => formatter.write_str("decimal arithmetic overflow"),
        }
    }
}

impl std::error::Error for FinanceError {}

/// `(actual - estimate) / |estimate| × 100`, rounded to one decimal place.
/// A zero estimate has no meaningful percentage surprise and returns `None`.
pub fn surprise_percent(
    actual: Decimal,
    estimate: Decimal,
) -> Result<Option<Decimal>, FinanceError> {
    if estimate.is_zero() {
        return Ok(None);
    }
    let delta = actual
        .checked_sub(estimate)
        .ok_or(FinanceError::ArithmeticOverflow)?;
    let ratio = delta
        .checked_div(estimate.abs())
        .ok_or(FinanceError::ArithmeticOverflow)?;
    let percent = ratio
        .checked_mul(Decimal::ONE_HUNDRED)
        .ok_or(FinanceError::ArithmeticOverflow)?;
    Ok(Some(percent.round_dp_with_strategy(
        1,
        RoundingStrategy::MidpointAwayFromZero,
    )))
}

/// Enterprise-value multiple × metric + net cash = implied equity value.
pub fn equity_value_from_multiple(
    metric: Money,
    multiple: Decimal,
    net_cash: Money,
) -> Result<Money, FinanceError> {
    if metric.currency != net_cash.currency {
        return Err(FinanceError::CurrencyMismatch);
    }
    let enterprise_value = metric
        .amount
        .checked_mul(multiple)
        .ok_or(FinanceError::ArithmeticOverflow)?;
    let equity_value = enterprise_value
        .checked_add(net_cash.amount)
        .ok_or(FinanceError::ArithmeticOverflow)?;
    Ok(Money::new(equity_value, metric.currency))
}

/// Subtract two same-currency money amounts (e.g. market value − cost value,
/// or a position's current price − its average cost, both expressed as Money
/// in the position's currency).
pub fn subtract(a: Money, b: Money) -> Result<Money, FinanceError> {
    if a.currency != b.currency {
        return Err(FinanceError::CurrencyMismatch);
    }
    let amount = a
        .amount
        .checked_sub(b.amount)
        .ok_or(FinanceError::ArithmeticOverflow)?;
    Ok(Money::new(amount, a.currency))
}

/// Multiply a money amount by a plain decimal factor (e.g. price × shares).
pub fn multiply(value: Money, factor: Decimal) -> Result<Money, FinanceError> {
    let amount = value
        .amount
        .checked_mul(factor)
        .ok_or(FinanceError::ArithmeticOverflow)?;
    Ok(Money::new(amount, value.currency))
}

/// Signed ratio of two same-currency money amounts (e.g. an unrealized-gain
/// numerator over its cost-basis denominator) — `None` when the denominator
/// is zero rather than producing an infinite/NaN ratio.
pub fn ratio(numerator: Money, denominator: Money) -> Result<Option<Decimal>, FinanceError> {
    if numerator.currency != denominator.currency {
        return Err(FinanceError::CurrencyMismatch);
    }
    if denominator.amount.is_zero() {
        return Ok(None);
    }
    let value = numerator
        .amount
        .checked_div(denominator.amount)
        .ok_or(FinanceError::ArithmeticOverflow)?;
    Ok(Some(value))
}

/// Convert total equity value into a per-share value with explicit precision.
pub fn per_share(
    equity_value: Money,
    diluted_shares: Decimal,
    decimal_places: u32,
) -> Result<Money, FinanceError> {
    if diluted_shares <= Decimal::ZERO {
        return Err(FinanceError::NonPositiveShares);
    }
    let value = equity_value
        .amount
        .checked_div(diluted_shares)
        .ok_or(FinanceError::ArithmeticOverflow)?
        .round_dp_with_strategy(decimal_places, RoundingStrategy::MidpointAwayFromZero);
    Ok(Money::new(value, equity_value.currency))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn earnings_surprise_has_exact_decimal_rounding() {
        assert_eq!(
            surprise_percent(Decimal::new(105, 2), Decimal::new(100, 2)),
            Ok(Some(Decimal::new(50, 1)))
        );
        assert_eq!(
            surprise_percent(Decimal::new(95, 2), Decimal::new(100, 2)),
            Ok(Some(Decimal::new(-50, 1)))
        );
        assert_eq!(surprise_percent(Decimal::ONE, Decimal::ZERO), Ok(None));
    }

    #[test]
    fn valuation_keeps_currency_and_never_uses_binary_float() {
        let revenue = Money::new(Decimal::new(650_000_000, 0), Currency::Usd);
        let net_cash = Money::new(Decimal::new(1_390_000_000, 0), Currency::Usd);
        let equity = equity_value_from_multiple(revenue, Decimal::new(10, 0), net_cash).unwrap();
        let price = per_share(equity, Decimal::new(550_000_000, 0), 2).unwrap();
        assert_eq!(equity.amount(), Decimal::new(7_890_000_000, 0));
        assert_eq!(price, Money::new(Decimal::new(1435, 2), Currency::Usd));
    }

    #[test]
    fn invalid_financial_dimensions_fail_loudly() {
        let usd = Money::new(Decimal::ONE, Currency::Usd);
        let hkd = Money::new(Decimal::ONE, Currency::Hkd);
        assert_eq!(
            equity_value_from_multiple(usd, Decimal::ONE, hkd),
            Err(FinanceError::CurrencyMismatch)
        );
        assert_eq!(
            per_share(usd, Decimal::ZERO, 2),
            Err(FinanceError::NonPositiveShares)
        );
    }

    #[test]
    fn position_pnl_uses_exact_decimal_arithmetic() {
        // 100 shares @ 317.31 vs. cost basis 280.00 — mirrors
        // 组合层的 enrich_position 规则：缺价格时保持未核到，不伪造盈亏。
        // binary float.
        let price = Money::new(Decimal::new(31731, 2), Currency::Usd);
        let avg_cost = Money::new(Decimal::new(28000, 2), Currency::Usd);
        let shares = Decimal::new(100, 0);

        let market_value = multiply(price, shares).unwrap();
        let cost_value = multiply(avg_cost, shares).unwrap();
        let unrealized_pnl = subtract(market_value, cost_value).unwrap();
        assert_eq!(market_value.amount(), Decimal::new(3173100, 2));
        assert_eq!(cost_value.amount(), Decimal::new(2800000, 2));
        assert_eq!(unrealized_pnl.amount(), Decimal::new(373100, 2));

        let gain = subtract(price, avg_cost).unwrap();
        let return_pct = ratio(gain, avg_cost).unwrap().unwrap();
        // 37.31 / 280.00 = 0.13325 exactly
        assert_eq!(return_pct, Decimal::new(13325, 5));
    }

    #[test]
    fn ratio_rejects_currency_mismatch_and_zero_denominator() {
        let usd = Money::new(Decimal::ONE, Currency::Usd);
        let hkd = Money::new(Decimal::ONE, Currency::Hkd);
        assert_eq!(ratio(usd, hkd), Err(FinanceError::CurrencyMismatch));
        assert_eq!(subtract(usd, hkd), Err(FinanceError::CurrencyMismatch));
        assert_eq!(
            ratio(usd, Money::new(Decimal::ZERO, Currency::Usd)),
            Ok(None)
        );
    }
}
