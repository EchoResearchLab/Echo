use crate::{MarketRow, Pool, Result, with_tenant};
use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use sqlx::FromRow;

#[derive(Clone, Debug, FromRow)]
pub struct WatchRuleRow {
    pub id: i64,
    pub ticker: String,
    pub kind: String,
    pub threshold: Decimal,
    pub metric: Option<String>,
    pub label: Option<String>,
}

#[derive(Clone, Debug, FromRow)]
pub struct ReminderProfileRow {
    pub ticker: String,
    pub company_name: Option<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, FromRow)]
pub struct EarningsCandidateRow {
    pub ticker: String,
    pub last_date: Option<String>,
    pub last_eps_estimate: Option<Decimal>,
    pub last_eps_actual: Option<Decimal>,
}

#[derive(Clone, Debug)]
pub struct PortfolioSnapshotResult {
    pub position_count: i32,
    pub missing_price: i32,
    pub missing_fx: i32,
    /// 未填成本价的持仓数。成本缺失时成本与盈亏都是断口，不是 0。
    pub missing_cost: i32,
    pub total_value_usd: Option<Decimal>,
    pub total_cost_usd: Option<Decimal>,
    pub total_pnl_usd: Option<Decimal>,
}

#[derive(FromRow)]
struct PortfolioSummaryRow {
    position_count: i32,
    missing_price: i32,
    missing_fx: i32,
    missing_cost: i32,
    total_value_usd: Option<Decimal>,
    total_cost_usd: Option<Decimal>,
    total_pnl_usd: Option<Decimal>,
}

pub struct OperationsRepository<'a> {
    pool: &'a Pool,
}

impl<'a> OperationsRepository<'a> {
    #[must_use]
    pub fn new(pool: &'a Pool) -> Self {
        Self { pool }
    }

    pub async fn user_ids(&self) -> Result<Vec<String>> {
        Ok(
            sqlx::query_scalar("SELECT id FROM users ORDER BY created_at, id")
                .fetch_all(self.pool)
                .await?,
        )
    }

    pub async fn tracked_tickers(&self) -> Result<Vec<String>> {
        Ok(sqlx::query_scalar(
            "SELECT ticker FROM (\
               SELECT ticker FROM portfolio_positions \
               UNION SELECT ticker FROM watchlist_prefs WHERE mode = 'add' \
               UNION SELECT ticker FROM company_profiles\
             ) tracked WHERE ticker NOT LIKE '%.SS' AND ticker NOT LIKE '%.SZ' ORDER BY ticker",
        )
        .fetch_all(self.pool)
        .await?)
    }

    pub async fn active_rules(&self, user_id: &str) -> Result<Vec<WatchRuleRow>> {
        let mut tx = with_tenant(self.pool, user_id).await?;
        let rows = sqlx::query_as::<_, WatchRuleRow>(
            "SELECT id, ticker, kind, threshold, metric, label FROM watch_rules \
             WHERE user_id = $1 AND active = true ORDER BY ticker, id",
        )
        .bind(user_id)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows)
    }

    pub async fn mark_rule_triggered(&self, user_id: &str, id: i64) -> Result<()> {
        let mut tx = with_tenant(self.pool, user_id).await?;
        sqlx::query(
            "UPDATE watch_rules SET last_triggered_at = now() WHERE user_id = $1 AND id = $2",
        )
        .bind(user_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// 过去 `hours` 小时内触发过的有效监控条件计数——供简报统计"本轮新增触线"用，
    /// 不返回明细（明细走通知列表，避免同一份触发在简报与通知面板各说一套）。
    pub async fn recently_triggered_rule_count(&self, user_id: &str, hours: i64) -> Result<i64> {
        let mut tx = with_tenant(self.pool, user_id).await?;
        let count = sqlx::query_scalar(
            "SELECT count(*) FROM watch_rules WHERE user_id = $1 AND active = true \
             AND last_triggered_at >= now() - make_interval(hours => $2::int)",
        )
        .bind(user_id)
        .bind(hours.clamp(1, 24 * 30) as i32)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(count)
    }

    pub async fn latest_market(&self, ticker: &str) -> Result<Option<MarketRow>> {
        crate::MarketRepository::new(self.pool)
            .latest_snapshot(ticker)
            .await
    }

    pub async fn latest_fundamental_metric(
        &self,
        ticker: &str,
        metric: &str,
    ) -> Result<Option<Decimal>> {
        let expression = match metric {
            "gross_margin" => "CASE WHEN revenue <> 0 THEN gross_profit / revenue * 100 END",
            "net_margin" => "CASE WHEN revenue <> 0 THEN net_income / revenue * 100 END",
            "revenue_growth" => {
                "CASE WHEN revenue_prior <> 0 THEN (revenue - revenue_prior) / abs(revenue_prior) * 100 END"
            }
            "free_cash_flow" => "free_cash_flow",
            _ => return Ok(None),
        };
        let query = format!(
            "SELECT {expression} FROM hk_financials WHERE ticker = $1 \
             ORDER BY knowledge_time DESC, id DESC LIMIT 1"
        );
        Ok(sqlx::query_scalar(&query)
            .bind(ticker)
            .fetch_optional(self.pool)
            .await?
            .flatten())
    }

    pub async fn reminder_profiles(&self, user_id: &str) -> Result<Vec<ReminderProfileRow>> {
        let mut tx = with_tenant(self.pool, user_id).await?;
        let rows = sqlx::query_as::<_, ReminderProfileRow>(
            "SELECT ticker, company_name, updated_at FROM company_profiles \
             WHERE user_id = $1 AND thesis IS NOT NULL AND updated_at <= now() - interval '30 days'",
        )
        .bind(user_id)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows)
    }

    pub async fn earnings_candidates(&self) -> Result<Vec<EarningsCandidateRow>> {
        Ok(sqlx::query_as::<_, EarningsCandidateRow>(
            "SELECT ticker, last_date, last_eps_estimate, last_eps_actual FROM earnings_calendar \
             WHERE last_year IS NOT NULL AND last_quarter IS NOT NULL AND last_eps_actual IS NOT NULL",
        )
        .fetch_all(self.pool)
        .await?)
    }

    pub async fn profile_name(&self, user_id: &str, ticker: &str) -> Result<Option<String>> {
        let mut tx = with_tenant(self.pool, user_id).await?;
        let name = sqlx::query_scalar(
            "SELECT company_name FROM company_profiles WHERE user_id = $1 AND ticker = $2",
        )
        .bind(user_id)
        .bind(ticker)
        .fetch_optional(&mut *tx)
        .await?
        .flatten();
        tx.commit().await?;
        Ok(name)
    }

    pub async fn append_earnings_event(
        &self,
        user_id: &str,
        candidate: &EarningsCandidateRow,
        summary: &str,
    ) -> Result<()> {
        let mut tx = with_tenant(self.pool, user_id).await?;
        sqlx::query(
            "INSERT INTO profile_events (user_id, ticker, date, kind, summary) \
             SELECT $1, $2, COALESCE($3, ''), 'earnings_report', $4 \
             WHERE EXISTS (SELECT 1 FROM company_profiles WHERE user_id = $1 AND ticker = $2)",
        )
        .bind(user_id)
        .bind(&candidate.ticker)
        .bind(&candidate.last_date)
        .bind(summary)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn capture_portfolio_snapshot(
        &self,
        user_id: &str,
        date: NaiveDate,
        hkd_usd: Option<Decimal>,
    ) -> Result<PortfolioSnapshotResult> {
        let mut tx = with_tenant(self.pool, user_id).await?;
        let summary = sqlx::query_as::<_, PortfolioSummaryRow>(
            "WITH latest AS (\
               SELECT DISTINCT ON (ticker) ticker, price FROM market_snapshots \
               ORDER BY ticker, valid_time DESC, id DESC\
             ), priced AS (\
               SELECT p.shares, p.avg_cost, l.price, \
                      CASE WHEN p.ticker LIKE '%.HK' THEN $2::numeric ELSE 1::numeric END AS fx \
               FROM portfolio_positions p LEFT JOIN latest l ON l.ticker = p.ticker \
               WHERE p.user_id = $1 AND p.shares IS NOT NULL \
                 AND p.ticker NOT LIKE '%.SS' AND p.ticker NOT LIKE '%.SZ'\
             ) SELECT count(*)::int AS position_count, \
                      count(*) FILTER (WHERE price IS NULL)::int AS missing_price, \
                      count(*) FILTER (WHERE fx IS NULL)::int AS missing_fx, \
                      count(*) FILTER (WHERE avg_cost IS NULL)::int AS missing_cost, \
                      sum(price * shares * fx) AS total_value_usd, \
                      sum(avg_cost * shares * fx) AS total_cost_usd, \
                      sum((price - avg_cost) * shares * fx) AS total_pnl_usd \
               FROM priced",
        )
        .bind(user_id)
        .bind(hkd_usd)
        .fetch_one(&mut *tx)
        .await?;
        // 成本缺一条，整组的成本与盈亏就都不成立：SUM 会静默跳过 NULL 行，留下一个
        // 少算了几笔成本的"总成本"，比缺口更危险。这里显式抹成断口（红线 2）。
        let cost_grounded = summary.missing_cost == 0;
        let result = PortfolioSnapshotResult {
            position_count: summary.position_count,
            missing_price: summary.missing_price,
            missing_fx: summary.missing_fx,
            missing_cost: summary.missing_cost,
            total_value_usd: summary.total_value_usd,
            total_cost_usd: cost_grounded.then_some(summary.total_cost_usd).flatten(),
            total_pnl_usd: cost_grounded.then_some(summary.total_pnl_usd).flatten(),
        };
        // 快照是历史盈亏曲线的唯一底账：缺价、缺汇率或缺成本任一成立都不落库，
        // 宁可这一天没有点，也不能写一条把未填成本当 0 的虚高盈亏。
        if result.position_count > 0
            && result.missing_price == 0
            && result.missing_fx == 0
            && result.missing_cost == 0
        {
            sqlx::query(
                "INSERT INTO portfolio_snapshots \
                 (user_id, valid_time, total_value_usd, total_cost_usd, total_pnl_usd, position_count, knowledge_time) \
                 VALUES ($1, $2, $3, $4, $5, $6, now()) \
                 ON CONFLICT (user_id, valid_time) DO UPDATE SET \
                   total_value_usd = excluded.total_value_usd, total_cost_usd = excluded.total_cost_usd, \
                   total_pnl_usd = excluded.total_pnl_usd, position_count = excluded.position_count, \
                   knowledge_time = now()",
            )
            .bind(user_id)
            .bind(date)
            .bind(result.total_value_usd)
            .bind(result.total_cost_usd)
            .bind(result.total_pnl_usd)
            .bind(result.position_count)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(result)
    }
}
