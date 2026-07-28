//! 自选监控规则（watch_rules）纯逻辑：允许的规则种类、阈值方向与字段要求。
//! 不碰 DB/网络/时钟——落库与外部数据核对交给 `echo-application`/`echo-db`/`echo-worker`。

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleKind {
    /// 现价 <= 阈值。
    PriceBelow,
    /// 现价 >= 阈值。
    PriceAbove,
    /// 指定基本面指标（`metric`）<= 阈值。
    FundamentalBelow,
    /// 指定基本面指标（`metric`）>= 阈值。
    FundamentalAbove,
    /// 历史估值分位（0-100）<= 阈值，仅支持美股。
    ValuationPercentileBelow,
    /// 历史估值分位（0-100）>= 阈值，仅支持美股。
    ValuationPercentileAbove,
    /// 事件触发：该 ticker 有新的业绩事实（EPS actual）落库。
    EventEarnings,
}

impl RuleKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PriceBelow => "price_below",
            Self::PriceAbove => "price_above",
            Self::FundamentalBelow => "fundamental_below",
            Self::FundamentalAbove => "fundamental_above",
            Self::ValuationPercentileBelow => "valuation_percentile_below",
            Self::ValuationPercentileAbove => "valuation_percentile_above",
            Self::EventEarnings => "event_earnings",
        }
    }

    #[must_use]
    pub fn parse(input: &str) -> Option<Self> {
        match input {
            "price_below" => Some(Self::PriceBelow),
            "price_above" => Some(Self::PriceAbove),
            "fundamental_below" => Some(Self::FundamentalBelow),
            "fundamental_above" => Some(Self::FundamentalAbove),
            "valuation_percentile_below" => Some(Self::ValuationPercentileBelow),
            "valuation_percentile_above" => Some(Self::ValuationPercentileAbove),
            "event_earnings" => Some(Self::EventEarnings),
            _ => None,
        }
    }

    /// 需要 `metric` 字段（基本面指标名）才能核对的规则种类。
    #[must_use]
    pub fn requires_metric(self) -> bool {
        matches!(self, Self::FundamentalBelow | Self::FundamentalAbove)
    }

    /// 需要用户显式给出阈值的规则种类；事件触发没有数值阈值，落库时用占位 0。
    #[must_use]
    pub fn requires_threshold(self) -> bool {
        !matches!(self, Self::EventEarnings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_every_kind() {
        let kinds = [
            RuleKind::PriceBelow,
            RuleKind::PriceAbove,
            RuleKind::FundamentalBelow,
            RuleKind::FundamentalAbove,
            RuleKind::ValuationPercentileBelow,
            RuleKind::ValuationPercentileAbove,
            RuleKind::EventEarnings,
        ];
        for kind in kinds {
            assert_eq!(RuleKind::parse(kind.as_str()), Some(kind));
        }
    }

    #[test]
    fn rejects_unknown_kind() {
        assert_eq!(RuleKind::parse("teleport"), None);
    }

    #[test]
    fn only_fundamental_requires_metric() {
        assert!(RuleKind::FundamentalBelow.requires_metric());
        assert!(RuleKind::FundamentalAbove.requires_metric());
        assert!(!RuleKind::PriceBelow.requires_metric());
        assert!(!RuleKind::ValuationPercentileBelow.requires_metric());
        assert!(!RuleKind::EventEarnings.requires_metric());
    }

    #[test]
    fn only_event_earnings_skips_threshold() {
        assert!(!RuleKind::EventEarnings.requires_threshold());
        assert!(RuleKind::ValuationPercentileAbove.requires_threshold());
    }
}
