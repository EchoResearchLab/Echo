//! 分位数——全仓唯一实现。
//!
//! 同业锚点与历史 PE 序列都要取四分位，此前各写各的：`peers` 用「四舍五入取最近索引」，
//! `historical` 用 `values[len/2]`。取整法在小样本上会让相邻分位塌到同一个值（n=3 时
//! p25 与中位同取 `values[1]`，n=4 时中位与 p75 同取 `values[2]`），而免费档可比集恰好
//! 就是 3–5 家——库里四条同业锚点因此全部退化成一个点，显示出来像「同业估值高度一致」，
//! 实际是算法假象。两处口径不一还会让同一个 PE 序列在事实块与估值里给出不同的中位数。

use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

/// 线性插值分位（与 PostgreSQL `percentile_cont` 同口径）。`values` 必须已升序排好，
/// `fraction` 取 0–1。空切片返回 `None`——缺数就是缺数，不造零值。
#[must_use]
pub fn percentile_sorted(values: &[Decimal], fraction: Decimal) -> Option<Decimal> {
    let n = values.len();
    if n == 0 {
        return None;
    }
    if n == 1 {
        return Some(values[0]);
    }
    let pos = fraction * Decimal::from(n - 1);
    let floor = pos.floor();
    let lo_index = floor.to_usize().unwrap_or(0).min(n - 1);
    let hi_index = pos.ceil().to_usize().unwrap_or(0).min(n - 1);
    let (lo, hi) = (values[lo_index], values[hi_index]);
    if lo_index == hi_index {
        return Some(lo);
    }
    Some(lo + (hi - lo) * (pos - floor))
}

/// 四分位 `(p25, median, p75)`。`values` 必须已升序。
#[must_use]
pub fn quartiles_sorted(values: &[Decimal]) -> Option<(Decimal, Decimal, Decimal)> {
    Some((
        percentile_sorted(values, Decimal::new(25, 2))?,
        percentile_sorted(values, Decimal::new(50, 2))?,
        percentile_sorted(values, Decimal::new(75, 2))?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    /// 小样本不得让相邻分位塌到同一个值——那会把「可比公司太少」伪装成「估值高度一致」。
    #[test]
    fn small_samples_still_spread_across_quartiles() {
        let four = [dec!(10), dec!(20), dec!(30), dec!(40)];
        assert_eq!(
            quartiles_sorted(&four),
            Some((dec!(17.5), dec!(25), dec!(32.5)))
        );

        let three = [dec!(10), dec!(20), dec!(30)];
        assert_eq!(
            quartiles_sorted(&three),
            Some((dec!(15), dec!(20), dec!(25)))
        );
    }

    #[test]
    fn matches_percentile_cont_on_odd_sample() {
        let five = [dec!(10), dec!(20), dec!(30), dec!(40), dec!(50)];
        assert_eq!(
            quartiles_sorted(&five),
            Some((dec!(20), dec!(30), dec!(40)))
        );
    }

    #[test]
    fn degenerate_inputs_are_honest() {
        assert_eq!(percentile_sorted(&[], Decimal::new(50, 2)), None);
        assert_eq!(
            quartiles_sorted(&[dec!(7)]),
            Some((dec!(7), dec!(7), dec!(7))),
            "单点序列的分位就是它自己，不外推"
        );
    }
}
