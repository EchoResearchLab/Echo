//! 财务衍生——从 `research.ts` 的 `deriveAnnualEps` / `toDomainSources` 里的比率衍生迁入。
//!
//! 全部用 `rust_decimal::Decimal` 定点（红线 4）。核心是 EPS 年化护栏：港股/A 股中报 EPS 是
//! **报告期累计值（YTD）**，不是 TTM；直接拿去反推 PE 会把 PE 抬高约 12/覆盖月数 倍。没有
//! HK/CN 的 TTM-PE 源，就用已取到的 filing 历史桥接出真实 TTM 净利：
//!   `TTM 净利 = 本期累计 + 上一完整财年 − 去年同期累计`，EPS 按同一比例缩放（不臆造股数）。
//! 拿不到上一财年行时诚实 `eps_annualized=false`，绝不猜。

use rust_decimal::Decimal;

/// `分子 / 分母 × 100`；分母缺失或为 0 返回 `None`（不把"缺失"当 0）。
#[must_use]
pub fn pct_of(numerator: Option<Decimal>, denominator: Option<Decimal>) -> Option<Decimal> {
    let (n, d) = (numerator?, denominator?);
    if d.is_zero() {
        return None;
    }
    Some((n / d) * Decimal::ONE_HUNDRED)
}

/// `(当期 − 上期) / |上期| × 100`；上期缺失或为 0 返回 `None`。
#[must_use]
pub fn pct_change(current: Option<Decimal>, prior: Option<Decimal>) -> Option<Decimal> {
    let (c, p) = (current?, prior?);
    if p.is_zero() {
        return None;
    }
    Some(((c - p) / p.abs()) * Decimal::ONE_HUNDRED)
}

/// 一行 filing（newest first），只取年化 EPS 需要的字段。
#[derive(Clone, Debug, Default)]
pub struct FilingRow {
    /// `"FY"` 表示完整财年；其它（Q1/H1/9M 等）为报告期累计。
    pub period_type: Option<String>,
    pub eps: Option<Decimal>,
    pub net_income: Option<Decimal>,
    /// filing 自带的去年同期比较值。
    pub net_income_prior: Option<Decimal>,
}

impl FilingRow {
    fn is_fy(&self) -> bool {
        self.period_type.as_deref() == Some("FY")
    }
}

/// 年化 EPS 结果。`annualized=false` 表示这是报告期累计值，**下游禁止用它反推 PE**。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnnualEps {
    pub eps: Option<Decimal>,
    pub annualized: bool,
}

/// 从 filing 历史（newest first，最多 4 行）桥接年化 EPS（`deriveAnnualEps` 忠实迁入）。
#[must_use]
pub fn derive_annual_eps(rows: &[FilingRow]) -> AnnualEps {
    let Some(latest) = rows.first() else {
        return AnnualEps {
            eps: None,
            annualized: true,
        };
    };
    let Some(eps) = latest.eps else {
        return AnnualEps {
            eps: None,
            annualized: true,
        };
    };
    if latest.is_fy() {
        return AnnualEps {
            eps: Some(eps),
            annualized: true,
        };
    }
    let prior_fy = rows.iter().skip(1).find(|r| r.is_fy());
    if let (Some(net_income), Some(net_income_prior), Some(prior_fy)) =
        (latest.net_income, latest.net_income_prior, prior_fy)
    {
        if net_income > Decimal::ZERO {
            if let Some(prior_fy_ni) = prior_fy.net_income {
                let ttm = net_income + prior_fy_ni - net_income_prior;
                if ttm > Decimal::ZERO {
                    return AnnualEps {
                        eps: Some(eps * (ttm / net_income)),
                        annualized: true,
                    };
                }
            }
        }
    }
    // 拿不到上一财年行/桥接不出正 TTM → 诚实标未年化，不猜。
    AnnualEps {
        eps: Some(eps),
        annualized: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn fy_eps_is_already_annual() {
        let rows = [FilingRow {
            period_type: Some("FY".into()),
            eps: Some(dec!(6.0)),
            ..Default::default()
        }];
        assert_eq!(
            derive_annual_eps(&rows),
            AnnualEps {
                eps: Some(dec!(6.0)),
                annualized: true
            }
        );
    }

    #[test]
    fn interim_without_prior_fy_is_not_annualized() {
        // 只有一行中报、没有上一财年 → eps_annualized=false（下游禁止反推 PE）。
        let rows = [FilingRow {
            period_type: Some("H1".into()),
            eps: Some(dec!(2.0)),
            net_income: Some(dec!(100)),
            net_income_prior: Some(dec!(90)),
        }];
        let r = derive_annual_eps(&rows);
        assert_eq!(r.eps, Some(dec!(2.0)));
        assert!(!r.annualized);
    }

    #[test]
    fn interim_bridges_to_ttm_with_prior_fy() {
        // 本期累计净利 120，去年同期 100，上一完整财年 200 → TTM = 120+200-100 = 220；
        // EPS 按 220/120 缩放。
        let rows = [
            FilingRow {
                period_type: Some("H1".into()),
                eps: Some(dec!(2.0)),
                net_income: Some(dec!(120)),
                net_income_prior: Some(dec!(100)),
            },
            FilingRow {
                period_type: Some("FY".into()),
                eps: Some(dec!(3.5)),
                net_income: Some(dec!(200)),
                net_income_prior: None,
            },
        ];
        let r = derive_annual_eps(&rows);
        assert!(r.annualized);
        // 2.0 × (220/120) = 3.6666...
        assert_eq!(r.eps.unwrap().round_dp(4), dec!(3.6667));
    }

    #[test]
    fn pct_helpers_treat_missing_as_none_not_zero() {
        assert_eq!(pct_of(Some(dec!(30)), Some(dec!(100))), Some(dec!(30)));
        assert_eq!(pct_of(Some(dec!(30)), None), None);
        assert_eq!(pct_of(Some(dec!(30)), Some(dec!(0))), None);
        assert_eq!(pct_change(Some(dec!(110)), Some(dec!(100))), Some(dec!(10)));
        // 分母取绝对值：(110-(-100))/|−100|×100 = 210（忠实 JS 口径）。
        assert_eq!(
            pct_change(Some(dec!(110)), Some(dec!(-100))),
            Some(dec!(210))
        );
    }
}
