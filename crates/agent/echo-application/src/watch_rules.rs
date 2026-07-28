//! 自选监控规则用例：种类/阈值校验（`echo-domain::RuleKind`）+ ticker 已核实建档校验，
//! 再落库（`echo-db::WatchRulesRepository`）。API 边界只做参数解析，不重复这层校验。

use echo_db::{CompanyRepository, NewWatchRule, Pool, WatchRuleDetailRow, WatchRulesRepository};
use echo_domain::RuleKind;
use rust_decimal::Decimal;

#[derive(Debug, thiserror::Error)]
pub enum WatchRuleError {
    #[error("未知的规则种类：{0}")]
    UnknownKind(String),
    #[error("该规则种类需要显式阈值")]
    MissingThreshold,
    #[error("该规则种类需要 metric 字段")]
    MissingMetric,
    #[error("ticker 尚未核实建档，请先完成公司解析")]
    UnverifiedTicker,
    #[error(transparent)]
    Db(#[from] echo_db::DbError),
}

pub struct WatchRuleService<'a> {
    pool: &'a Pool,
}

impl<'a> WatchRuleService<'a> {
    #[must_use]
    pub fn new(pool: &'a Pool) -> Self {
        Self { pool }
    }

    pub async fn create(
        &self,
        user_id: &str,
        ticker: &str,
        kind: &str,
        threshold: Option<Decimal>,
        metric: Option<&str>,
        label: Option<&str>,
    ) -> Result<WatchRuleDetailRow, WatchRuleError> {
        let parsed =
            RuleKind::parse(kind).ok_or_else(|| WatchRuleError::UnknownKind(kind.to_string()))?;
        if parsed.requires_metric() && metric.is_none_or(|value| value.trim().is_empty()) {
            return Err(WatchRuleError::MissingMetric);
        }
        let threshold = if parsed.requires_threshold() {
            threshold.ok_or(WatchRuleError::MissingThreshold)?
        } else {
            Decimal::ZERO
        };
        let normalized = echo_db::normalize_ticker(ticker);
        if CompanyRepository::new(self.pool)
            .by_ticker(&normalized)
            .await?
            .is_none()
        {
            return Err(WatchRuleError::UnverifiedTicker);
        }
        let row = WatchRulesRepository::new(self.pool)
            .create(
                user_id,
                &NewWatchRule {
                    ticker: &normalized,
                    kind: parsed.as_str(),
                    threshold,
                    metric,
                    label,
                },
            )
            .await?;
        Ok(row)
    }

    pub async fn list(&self, user_id: &str) -> Result<Vec<WatchRuleDetailRow>, WatchRuleError> {
        Ok(WatchRulesRepository::new(self.pool).list(user_id).await?)
    }

    pub async fn delete(&self, user_id: &str, id: i64) -> Result<bool, WatchRuleError> {
        Ok(WatchRulesRepository::new(self.pool)
            .delete(user_id, id)
            .await?)
    }
}
