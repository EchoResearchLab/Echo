use crate::{Pool, Result, with_tenant};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::Value;
use sqlx::FromRow;

#[must_use]
pub fn normalize_ticker(input: &str) -> String {
    input.trim().to_uppercase()
}

#[derive(Clone, Debug, FromRow)]
pub struct CompanySearchRow {
    pub ticker: String,
    pub name_zh: String,
    pub name_en: Option<String>,
    pub sector: Option<String>,
    pub industry: Option<String>,
    pub has_portrait: bool,
}

impl super::super::CompanyRepository<'_> {
    pub async fn search(&self, query: &str, limit: i64) -> Result<Vec<CompanySearchRow>> {
        let pattern = format!("%{}%", query.trim());
        Ok(sqlx::query_as::<_, CompanySearchRow>(
            "SELECT c.ticker, c.name_zh, c.name_en, c.sector, c.industry, \
                    (d.ticker IS NOT NULL) AS has_portrait \
             FROM companies c LEFT JOIN company_details d ON d.ticker = c.ticker \
             WHERE c.ticker ILIKE $1 OR c.name_zh ILIKE $1 OR c.name_en ILIKE $1 \
             ORDER BY CASE WHEN c.ticker ILIKE $1 THEN 0 ELSE 1 END, \
                      CASE WHEN c.name_zh ILIKE $1 THEN 0 ELSE 1 END, c.name_zh ASC \
             LIMIT $2",
        )
        .bind(pattern)
        .bind(limit.clamp(1, 100))
        .fetch_all(self.pool)
        .await?)
    }

    /// 仅供已经通过真实供应商核实的 ticker 建档。
    pub async fn ensure(
        &self,
        ticker: &str,
        name_zh: Option<&str>,
        name_en: Option<&str>,
        sector: Option<&str>,
        industry: Option<&str>,
    ) -> Result<()> {
        let ticker = normalize_ticker(ticker);
        let is_us = !ticker.contains('.');
        sqlx::query(
            "INSERT INTO companies \
             (ticker, name_zh, name_en, sector, industry, exchange, currency) \
             VALUES ($1, COALESCE(NULLIF($2, ''), $1), $3, $4, $5, $6, $7) \
             ON CONFLICT (ticker) DO NOTHING",
        )
        .bind(&ticker)
        .bind(name_zh)
        .bind(name_en.or(name_zh).filter(|_| is_us))
        .bind(sector)
        .bind(industry)
        .bind(if is_us { "US" } else { "HKEX" })
        .bind(if is_us { "USD" } else { "HKD" })
        .execute(self.pool)
        .await?;
        Ok(())
    }
}

#[derive(Clone, Debug, FromRow)]
pub struct WatchEntryRow {
    pub ticker: String,
    pub company_name: Option<String>,
    pub mode: String,
    pub created_at: DateTime<Utc>,
}

pub struct WatchlistRepository<'a> {
    pool: &'a Pool,
}

impl<'a> WatchlistRepository<'a> {
    #[must_use]
    pub fn new(pool: &'a Pool) -> Self {
        Self { pool }
    }

    pub async fn list(&self, user_id: &str) -> Result<Vec<WatchEntryRow>> {
        let mut tx = with_tenant(self.pool, user_id).await?;
        let rows = sqlx::query_as::<_, WatchEntryRow>(
            "SELECT ticker, company_name, mode, created_at FROM watchlist_prefs \
             WHERE user_id = $1 ORDER BY created_at DESC",
        )
        .bind(user_id)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows)
    }

    pub async fn set(
        &self,
        user_id: &str,
        ticker: &str,
        company_name: Option<&str>,
        mode: &str,
    ) -> Result<bool> {
        let ticker = normalize_ticker(ticker);
        if ticker.is_empty() || !matches!(mode, "add" | "hide") {
            return Ok(false);
        }
        let mut tx = with_tenant(self.pool, user_id).await?;
        sqlx::query(
            "INSERT INTO watchlist_prefs (user_id, ticker, company_name, mode) \
             VALUES ($1, $2, $3, $4) \
             ON CONFLICT (user_id, ticker) DO UPDATE SET \
               company_name = COALESCE(excluded.company_name, watchlist_prefs.company_name), \
               mode = excluded.mode, created_at = now()",
        )
        .bind(user_id)
        .bind(ticker)
        .bind(company_name)
        .bind(mode)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }
}

#[derive(Clone, Debug, FromRow)]
pub struct WatchRuleDetailRow {
    pub id: i64,
    pub ticker: String,
    pub kind: String,
    pub threshold: Decimal,
    pub metric: Option<String>,
    pub label: Option<String>,
    pub active: bool,
    pub created_at: DateTime<Utc>,
    pub last_triggered_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug)]
pub struct NewWatchRule<'a> {
    pub ticker: &'a str,
    pub kind: &'a str,
    pub threshold: Decimal,
    pub metric: Option<&'a str>,
    pub label: Option<&'a str>,
}

pub struct WatchRulesRepository<'a> {
    pool: &'a Pool,
}

impl<'a> WatchRulesRepository<'a> {
    #[must_use]
    pub fn new(pool: &'a Pool) -> Self {
        Self { pool }
    }

    pub async fn list(&self, user_id: &str) -> Result<Vec<WatchRuleDetailRow>> {
        let mut tx = with_tenant(self.pool, user_id).await?;
        let rows = sqlx::query_as::<_, WatchRuleDetailRow>(
            "SELECT id, ticker, kind, threshold, metric, label, active, created_at, last_triggered_at \
             FROM watch_rules WHERE user_id = $1 ORDER BY ticker, id",
        )
        .bind(user_id)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows)
    }

    /// 建规则前调用方须已核实 ticker 存在（`CompanyRepository::by_ticker`）——不在此仓储内隐式建档。
    pub async fn create(
        &self,
        user_id: &str,
        input: &NewWatchRule<'_>,
    ) -> Result<WatchRuleDetailRow> {
        let ticker = normalize_ticker(input.ticker);
        let mut tx = with_tenant(self.pool, user_id).await?;
        let row = sqlx::query_as::<_, WatchRuleDetailRow>(
            "INSERT INTO watch_rules \
             (user_id, ticker, kind, threshold, metric, label, source, active) \
             VALUES ($1, $2, $3, $4, $5, $6, 'user', true) \
             RETURNING id, ticker, kind, threshold, metric, label, active, created_at, last_triggered_at",
        )
        .bind(user_id)
        .bind(&ticker)
        .bind(input.kind)
        .bind(input.threshold)
        .bind(input.metric)
        .bind(input.label)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(row)
    }

    pub async fn delete(&self, user_id: &str, id: i64) -> Result<bool> {
        let mut tx = with_tenant(self.pool, user_id).await?;
        let result = sqlx::query("DELETE FROM watch_rules WHERE user_id = $1 AND id = $2")
            .bind(user_id)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(result.rows_affected() == 1)
    }
}

#[derive(Clone, Debug, FromRow)]
pub struct PortfolioPositionRow {
    pub ticker: String,
    pub company_name: Option<String>,
    pub shares: Option<Decimal>,
    pub avg_cost: Option<Decimal>,
    pub stop_loss: Option<Decimal>,
    pub take_profit: Option<Decimal>,
    pub note: Option<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Default)]
pub struct PortfolioUpsert {
    pub company_name: Option<String>,
    pub shares: Option<Decimal>,
    pub avg_cost: Option<Decimal>,
    pub stop_loss: Option<Decimal>,
    pub take_profit: Option<Decimal>,
    pub note: Option<String>,
}

pub struct PortfolioRepository<'a> {
    pool: &'a Pool,
}

impl<'a> PortfolioRepository<'a> {
    #[must_use]
    pub fn new(pool: &'a Pool) -> Self {
        Self { pool }
    }

    pub async fn list(&self, user_id: &str) -> Result<Vec<PortfolioPositionRow>> {
        let mut tx = with_tenant(self.pool, user_id).await?;
        let rows = sqlx::query_as::<_, PortfolioPositionRow>(
            "SELECT ticker, company_name, shares, avg_cost, stop_loss, take_profit, note, updated_at \
             FROM portfolio_positions WHERE user_id = $1 ORDER BY updated_at DESC",
        )
        .bind(user_id)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows)
    }

    pub async fn upsert(
        &self,
        user_id: &str,
        ticker: &str,
        patch: &PortfolioUpsert,
    ) -> Result<PortfolioPositionRow> {
        let ticker = normalize_ticker(ticker);
        let is_us = !ticker.contains('.');
        let mut tx = with_tenant(self.pool, user_id).await?;
        sqlx::query(
            "INSERT INTO companies (ticker, name_zh, name_en, exchange, currency) \
             VALUES ($1, COALESCE(NULLIF($2, ''), $1), CASE WHEN $3 THEN COALESCE(NULLIF($2, ''), $1) ELSE NULL END, $4, $5) \
             ON CONFLICT (ticker) DO NOTHING",
        )
        .bind(&ticker)
        .bind(&patch.company_name)
        .bind(is_us)
        .bind(if is_us { "US" } else { "HKEX" })
        .bind(if is_us { "USD" } else { "HKD" })
        .execute(&mut *tx)
        .await?;
        let row = sqlx::query_as::<_, PortfolioPositionRow>(
            "INSERT INTO portfolio_positions \
             (user_id, ticker, company_name, shares, avg_cost, stop_loss, take_profit, note, updated_at) \
             VALUES ($1, $2, COALESCE($3, $2), $4, $5, $6, $7, COALESCE($8, ''), now()) \
             ON CONFLICT (user_id, ticker) DO UPDATE SET \
               company_name = COALESCE($3, portfolio_positions.company_name), \
               shares = COALESCE($4, portfolio_positions.shares), \
               avg_cost = COALESCE($5, portfolio_positions.avg_cost), \
               stop_loss = COALESCE($6, portfolio_positions.stop_loss), \
               take_profit = COALESCE($7, portfolio_positions.take_profit), \
               note = COALESCE($8, portfolio_positions.note), updated_at = now() \
             RETURNING ticker, company_name, shares, avg_cost, stop_loss, take_profit, note, updated_at",
        )
        .bind(user_id)
        .bind(&ticker)
        .bind(&patch.company_name)
        .bind(patch.shares)
        .bind(patch.avg_cost)
        .bind(patch.stop_loss)
        .bind(patch.take_profit)
        .bind(&patch.note)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(row)
    }

    pub async fn delete(&self, user_id: &str, ticker: &str) -> Result<bool> {
        let mut tx = with_tenant(self.pool, user_id).await?;
        let result =
            sqlx::query("DELETE FROM portfolio_positions WHERE user_id = $1 AND ticker = $2")
                .bind(user_id)
                .bind(normalize_ticker(ticker))
                .execute(&mut *tx)
                .await?;
        tx.commit().await?;
        Ok(result.rows_affected() == 1)
    }
}

#[derive(Clone, Debug, FromRow)]
pub struct UserPreferencesRow {
    pub onboarding_completed: bool,
    pub notify_digest: bool,
    pub notify_positions: bool,
    pub notify_falsify: bool,
    pub notify_review: bool,
    pub notify_earnings: bool,
    pub quiet_hours_start: Option<String>,
    pub quiet_hours_end: Option<String>,
}

impl Default for UserPreferencesRow {
    fn default() -> Self {
        Self {
            onboarding_completed: false,
            notify_digest: true,
            notify_positions: true,
            notify_falsify: true,
            notify_review: true,
            notify_earnings: true,
            quiet_hours_start: None,
            quiet_hours_end: None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct PreferencesPatch {
    pub onboarding_completed: Option<bool>,
    pub notify_digest: Option<bool>,
    pub notify_positions: Option<bool>,
    pub notify_falsify: Option<bool>,
    pub notify_review: Option<bool>,
    pub notify_earnings: Option<bool>,
    pub quiet_hours_start: Option<String>,
    pub quiet_hours_end: Option<String>,
}

pub struct PreferencesRepository<'a> {
    pool: &'a Pool,
}

impl<'a> PreferencesRepository<'a> {
    #[must_use]
    pub fn new(pool: &'a Pool) -> Self {
        Self { pool }
    }

    pub async fn get(&self, user_id: &str) -> Result<UserPreferencesRow> {
        let mut tx = with_tenant(self.pool, user_id).await?;
        let row = sqlx::query_as::<_, UserPreferencesRow>(
            "SELECT onboarding_completed, notify_digest, notify_positions, notify_falsify, \
                    notify_review, notify_earnings, quiet_hours_start, quiet_hours_end \
             FROM user_preferences WHERE user_id = $1",
        )
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(row.unwrap_or_default())
    }

    pub async fn update(
        &self,
        user_id: &str,
        patch: &PreferencesPatch,
    ) -> Result<UserPreferencesRow> {
        let mut tx = with_tenant(self.pool, user_id).await?;
        let row = sqlx::query_as::<_, UserPreferencesRow>(
            "INSERT INTO user_preferences \
             (user_id, onboarding_completed, notify_digest, notify_positions, notify_falsify, \
              notify_review, notify_earnings, quiet_hours_start, quiet_hours_end, updated_at) \
             VALUES ($1, COALESCE($2, false), COALESCE($3, true), COALESCE($4, true), \
                     COALESCE($5, true), COALESCE($6, true), COALESCE($7, true), \
                     NULLIF($8, ''), NULLIF($9, ''), now()) \
             ON CONFLICT (user_id) DO UPDATE SET \
               onboarding_completed = COALESCE($2, user_preferences.onboarding_completed), \
               notify_digest = COALESCE($3, user_preferences.notify_digest), \
               notify_positions = COALESCE($4, user_preferences.notify_positions), \
               notify_falsify = COALESCE($5, user_preferences.notify_falsify), \
               notify_review = COALESCE($6, user_preferences.notify_review), \
               notify_earnings = COALESCE($7, user_preferences.notify_earnings), \
               quiet_hours_start = CASE WHEN $8 IS NULL THEN user_preferences.quiet_hours_start \
                                        ELSE NULLIF($8, '') END, \
               quiet_hours_end = CASE WHEN $9 IS NULL THEN user_preferences.quiet_hours_end \
                                      ELSE NULLIF($9, '') END, updated_at = now() \
             RETURNING onboarding_completed, notify_digest, notify_positions, notify_falsify, \
                       notify_review, notify_earnings, quiet_hours_start, quiet_hours_end",
        )
        .bind(user_id)
        .bind(patch.onboarding_completed)
        .bind(patch.notify_digest)
        .bind(patch.notify_positions)
        .bind(patch.notify_falsify)
        .bind(patch.notify_review)
        .bind(patch.notify_earnings)
        .bind(&patch.quiet_hours_start)
        .bind(&patch.quiet_hours_end)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(row)
    }
}

#[derive(Clone, Debug, FromRow)]
pub struct NotificationRow {
    pub id: i64,
    pub kind: String,
    pub title: String,
    pub body: Option<String>,
    pub ticker: Option<String>,
    pub payload: Option<Value>,
    pub created_at: DateTime<Utc>,
    pub read_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug)]
pub struct NewNotification<'a> {
    pub kind: &'a str,
    pub title: &'a str,
    pub body: &'a str,
    pub ticker: Option<&'a str>,
    pub payload: Option<&'a Value>,
    pub dedupe_key: Option<&'a str>,
    pub dedupe_window_hours: i64,
}

pub struct NotificationsRepository<'a> {
    pool: &'a Pool,
}

impl<'a> NotificationsRepository<'a> {
    #[must_use]
    pub fn new(pool: &'a Pool) -> Self {
        Self { pool }
    }

    pub async fn list(&self, user_id: &str, limit: i64) -> Result<Vec<NotificationRow>> {
        let mut tx = with_tenant(self.pool, user_id).await?;
        let rows = sqlx::query_as::<_, NotificationRow>(
            "SELECT id, kind, title, body, ticker, payload, created_at, read_at FROM notifications \
             WHERE user_id = $1 ORDER BY id DESC LIMIT $2",
        )
        .bind(user_id)
        .bind(limit.clamp(1, 100))
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows)
    }

    /// 通知产生的唯一咽喉：偏好、免打扰与去重都在这里执行，调用方不能绕开。
    pub async fn insert(&self, user_id: &str, input: &NewNotification<'_>) -> Result<Option<i64>> {
        let preferences = PreferencesRepository::new(self.pool).get(user_id).await?;
        let enabled = match input.kind {
            "event_digest" => preferences.notify_digest,
            "position_alert" => preferences.notify_positions,
            "falsify_alert" => preferences.notify_falsify,
            "review_reminder" => preferences.notify_review,
            "earnings_review" => preferences.notify_earnings,
            _ => true,
        };
        if !enabled {
            return Ok(None);
        }
        let urgent = matches!(input.kind, "position_alert" | "falsify_alert");
        if !urgent && in_quiet_hours(&preferences, Utc::now()) {
            return Ok(None);
        }
        let mut tx = with_tenant(self.pool, user_id).await?;
        if let Some(key) = input.dedupe_key {
            let duplicate: Option<i64> = sqlx::query_scalar(
                "SELECT id FROM notifications WHERE user_id = $1 AND dedupe_key = $2 \
                 AND created_at >= now() - make_interval(hours => $3::int) LIMIT 1",
            )
            .bind(user_id)
            .bind(key)
            .bind(input.dedupe_window_hours.clamp(1, 24 * 365) as i32)
            .fetch_optional(&mut *tx)
            .await?;
            if duplicate.is_some() {
                tx.commit().await?;
                return Ok(None);
            }
        }
        let id = sqlx::query_scalar(
            "INSERT INTO notifications \
             (user_id, kind, title, body, ticker, payload, dedupe_key) \
             VALUES ($1, $2, left($3, 300), left($4, 4000), $5, $6, $7) RETURNING id",
        )
        .bind(user_id)
        .bind(input.kind)
        .bind(input.title)
        .bind(input.body)
        .bind(input.ticker)
        .bind(input.payload)
        .bind(input.dedupe_key)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Some(id))
    }

    pub async fn unread_count(&self, user_id: &str) -> Result<i64> {
        let mut tx = with_tenant(self.pool, user_id).await?;
        let count = sqlx::query_scalar(
            "SELECT count(*) FROM notifications WHERE user_id = $1 AND read_at IS NULL",
        )
        .bind(user_id)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(count)
    }

    pub async fn mark_read(&self, user_id: &str, id: Option<i64>) -> Result<u64> {
        let mut tx = with_tenant(self.pool, user_id).await?;
        let result = match id {
            Some(id) => {
                sqlx::query(
                    "UPDATE notifications SET read_at = now() \
                 WHERE user_id = $1 AND id = $2 AND read_at IS NULL",
                )
                .bind(user_id)
                .bind(id)
                .execute(&mut *tx)
                .await?
            }
            None => {
                sqlx::query(
                    "UPDATE notifications SET read_at = now() \
                 WHERE user_id = $1 AND read_at IS NULL",
                )
                .bind(user_id)
                .execute(&mut *tx)
                .await?
            }
        };
        tx.commit().await?;
        Ok(result.rows_affected())
    }
}

fn in_quiet_hours(preferences: &UserPreferencesRow, now: DateTime<Utc>) -> bool {
    let (Some(start), Some(end)) = (
        preferences.quiet_hours_start.as_deref(),
        preferences.quiet_hours_end.as_deref(),
    ) else {
        return false;
    };
    let hhmm = now.format("%H:%M").to_string();
    if start <= end {
        hhmm.as_str() >= start && hhmm.as_str() < end
    } else {
        hhmm.as_str() >= start || hhmm.as_str() < end
    }
}

#[derive(Clone, Debug, FromRow)]
pub struct ResearchSessionRow {
    pub id: String,
    pub ticker: Option<String>,
    pub title: Option<String>,
    pub question: Option<String>,
    pub conversation_id: Option<String>,
    pub status: String,
    pub report_markdown: Option<String>,
    pub rating: Option<String>,
    pub confidence: Option<String>,
    pub decision_panel: Option<Value>,
    pub full_research: Option<String>,
    pub data_sources: Option<Value>,
    pub thread_json: Option<Value>,
    pub turn_count: Option<i32>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, FromRow)]
pub struct ResearchSessionSummaryRow {
    pub id: String,
    pub ticker: Option<String>,
    pub title: Option<String>,
    pub question: Option<String>,
    pub conversation_id: Option<String>,
    pub status: String,
    pub rating: Option<String>,
    pub confidence: Option<String>,
    pub turn_count: Option<i32>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub company_name: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct SaveResearchSession {
    pub id: Option<String>,
    pub ticker: String,
    pub company_name: Option<String>,
    pub title: Option<String>,
    pub question: Option<String>,
    pub conversation_id: Option<String>,
    pub status: Option<String>,
    pub report_markdown: Option<String>,
    pub rating: Option<String>,
    pub confidence: Option<String>,
    pub decision_panel: Option<Value>,
    pub full_research: Option<String>,
    pub data_sources: Option<Value>,
    pub thread: Option<Value>,
    pub turn_count: Option<i32>,
}

pub struct ResearchSessionRepository<'a> {
    pool: &'a Pool,
}

impl<'a> ResearchSessionRepository<'a> {
    #[must_use]
    pub fn new(pool: &'a Pool) -> Self {
        Self { pool }
    }

    pub async fn get(&self, user_id: &str, id: &str) -> Result<Option<ResearchSessionRow>> {
        let mut tx = with_tenant(self.pool, user_id).await?;
        let row = sqlx::query_as::<_, ResearchSessionRow>(
            "SELECT id, ticker, title, question, conversation_id, status, report_markdown, rating, \
                    confidence, decision_panel, full_research, data_sources, thread_json, turn_count, \
                    created_at, updated_at FROM research_sessions WHERE user_id = $1 AND id = $2",
        )
        .bind(user_id)
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(row)
    }

    pub async fn list(
        &self,
        user_id: &str,
        ticker: Option<&str>,
        limit: i64,
    ) -> Result<Vec<ResearchSessionSummaryRow>> {
        let mut tx = with_tenant(self.pool, user_id).await?;
        let rows = sqlx::query_as::<_, ResearchSessionSummaryRow>(
            "SELECT s.id, s.ticker, s.title, s.question, s.conversation_id, s.status, s.rating, \
                    s.confidence, s.turn_count, s.created_at, s.updated_at, c.name_zh AS company_name \
             FROM research_sessions s LEFT JOIN companies c ON c.ticker = s.ticker \
             WHERE s.user_id = $1 AND ($2::text IS NULL OR s.ticker = $2) \
             ORDER BY s.updated_at DESC, s.id DESC LIMIT $3",
        )
        .bind(user_id)
        .bind(ticker.map(normalize_ticker))
        .bind(limit.clamp(1, 100))
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows)
    }

    pub async fn save(&self, user_id: &str, input: &SaveResearchSession) -> Result<String> {
        let ticker = normalize_ticker(&input.ticker);
        let id = input
            .id
            .clone()
            .unwrap_or_else(|| format!("s_{}", uuid::Uuid::new_v4()));
        let is_us = !ticker.contains('.');
        let mut tx = with_tenant(self.pool, user_id).await?;
        sqlx::query(
            "INSERT INTO companies (ticker, name_zh, name_en, exchange, currency) \
             VALUES ($1, COALESCE(NULLIF($2, ''), $1), CASE WHEN $3 THEN COALESCE(NULLIF($2, ''), $1) ELSE NULL END, $4, $5) \
             ON CONFLICT (ticker) DO NOTHING",
        )
        .bind(&ticker)
        .bind(&input.company_name)
        .bind(is_us)
        .bind(if is_us { "US" } else { "HKEX" })
        .bind(if is_us { "USD" } else { "HKD" })
        .execute(&mut *tx)
        .await?;
        let result = sqlx::query(
            "INSERT INTO research_sessions \
             (id, user_id, ticker, title, question, conversation_id, status, report_markdown, rating, \
              confidence, decision_panel, full_research, data_sources, thread_json, turn_count, updated_at) \
             VALUES ($1, $2, $3, COALESCE($4, $5, ''), COALESCE($5, ''), COALESCE($6, $1), \
                     COALESCE($7, 'completed'), $8, $9, $10, $11, $12, $13, $14, $15, now()) \
             ON CONFLICT (id) DO UPDATE SET \
               ticker = excluded.ticker, title = COALESCE(NULLIF(excluded.title, ''), research_sessions.title), \
               question = COALESCE(NULLIF(excluded.question, ''), research_sessions.question), \
               status = excluded.status, report_markdown = COALESCE(excluded.report_markdown, research_sessions.report_markdown), \
               rating = COALESCE(excluded.rating, research_sessions.rating), confidence = COALESCE(excluded.confidence, research_sessions.confidence), \
               decision_panel = COALESCE(excluded.decision_panel, research_sessions.decision_panel), \
               full_research = COALESCE(excluded.full_research, research_sessions.full_research), \
               data_sources = COALESCE(excluded.data_sources, research_sessions.data_sources), \
               thread_json = COALESCE(excluded.thread_json, research_sessions.thread_json), \
               turn_count = COALESCE(excluded.turn_count, research_sessions.turn_count), updated_at = now() \
             WHERE research_sessions.user_id = excluded.user_id",
        )
        .bind(&id)
        .bind(user_id)
        .bind(ticker)
        .bind(&input.title)
        .bind(&input.question)
        .bind(&input.conversation_id)
        .bind(&input.status)
        .bind(&input.report_markdown)
        .bind(&input.rating)
        .bind(&input.confidence)
        .bind(&input.decision_panel)
        .bind(&input.full_research)
        .bind(&input.data_sources)
        .bind(&input.thread)
        .bind(input.turn_count)
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() != 1 {
            return Err(crate::DbError::TenantConflict(id));
        }
        tx.commit().await?;
        Ok(id)
    }

    pub async fn delete(&self, user_id: &str, id: &str) -> Result<bool> {
        let mut tx = with_tenant(self.pool, user_id).await?;
        let result = sqlx::query("DELETE FROM research_sessions WHERE user_id = $1 AND id = $2")
            .bind(user_id)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn clear(&self, user_id: &str) -> Result<u64> {
        let mut tx = with_tenant(self.pool, user_id).await?;
        let result = sqlx::query("DELETE FROM research_sessions WHERE user_id = $1")
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(result.rows_affected())
    }
}

/// 公司档案摘要行（列表视图）。
#[derive(Clone, Debug, FromRow)]
pub struct CompanyProfileSummaryRow {
    pub ticker: String,
    pub company_name: Option<String>,
    pub research_status: Option<String>,
    pub confidence: Option<String>,
    pub turn_count: i32,
    pub updated_at: DateTime<Utc>,
}

/// 公司档案完整行（详情视图）。
#[derive(Clone, Debug, FromRow)]
pub struct CompanyProfileRow {
    pub ticker: String,
    pub company_name: Option<String>,
    pub thesis: Option<String>,
    pub research_status: Option<String>,
    pub confidence: Option<String>,
    pub bull: Option<Vec<String>>,
    pub bear: Option<Vec<String>>,
    pub monitors: Option<Vec<String>>,
    pub falsifiers: Option<Vec<String>>,
    pub valuation_method: Option<String>,
    pub valuation_bear: Option<Decimal>,
    pub valuation_base: Option<Decimal>,
    pub valuation_bull: Option<Decimal>,
    pub valuation_current_price: Option<Decimal>,
    pub profile_md: Option<String>,
    pub turn_count: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// 部分更新——每个字段 `None` 表示"本次不改"，与既有行合并（`COALESCE`）。清空一个
/// 数组/文本字段需显式传 `Some(vec![])`/`Some(String::new())`，不能靠省略字段做到。
#[derive(Clone, Debug, Default)]
pub struct CompanyProfileUpsert {
    pub company_name: Option<String>,
    pub thesis: Option<String>,
    pub research_status: Option<String>,
    pub confidence: Option<String>,
    pub bull: Option<Vec<String>>,
    pub bear: Option<Vec<String>>,
    pub monitors: Option<Vec<String>>,
    pub falsifiers: Option<Vec<String>>,
    pub valuation_method: Option<String>,
    pub valuation_bear: Option<Decimal>,
    pub valuation_base: Option<Decimal>,
    pub valuation_bull: Option<Decimal>,
    pub valuation_current_price: Option<Decimal>,
    pub profile_md: Option<String>,
}

pub struct CompanyProfileRepository<'a> {
    pool: &'a Pool,
}

impl<'a> CompanyProfileRepository<'a> {
    #[must_use]
    pub fn new(pool: &'a Pool) -> Self {
        Self { pool }
    }

    pub async fn list(&self, user_id: &str, limit: i64) -> Result<Vec<CompanyProfileSummaryRow>> {
        let mut tx = with_tenant(self.pool, user_id).await?;
        let rows = sqlx::query_as::<_, CompanyProfileSummaryRow>(
            "SELECT ticker, company_name, research_status, confidence, turn_count, updated_at \
             FROM company_profiles WHERE user_id = $1 ORDER BY updated_at DESC LIMIT $2",
        )
        .bind(user_id)
        .bind(limit.clamp(1, 200))
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows)
    }

    pub async fn get(&self, user_id: &str, ticker: &str) -> Result<Option<CompanyProfileRow>> {
        let mut tx = with_tenant(self.pool, user_id).await?;
        let row = sqlx::query_as::<_, CompanyProfileRow>(
            "SELECT ticker, company_name, thesis, research_status, confidence, bull, bear, \
                    monitors, falsifiers, valuation_method, valuation_bear, valuation_base, \
                    valuation_bull, valuation_current_price, profile_md, turn_count, \
                    created_at, updated_at \
             FROM company_profiles WHERE user_id = $1 AND ticker = $2",
        )
        .bind(user_id)
        .bind(normalize_ticker(ticker))
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(row)
    }

    /// 建档/更新并把 `turn_count` 加一——每次研究沉淀都算一轮判断迭代。
    pub async fn upsert(
        &self,
        user_id: &str,
        ticker: &str,
        patch: &CompanyProfileUpsert,
    ) -> Result<CompanyProfileRow> {
        let ticker = normalize_ticker(ticker);
        let mut tx = with_tenant(self.pool, user_id).await?;
        let row = sqlx::query_as::<_, CompanyProfileRow>(
            "INSERT INTO company_profiles \
             (user_id, ticker, company_name, thesis, research_status, confidence, bull, bear, \
              monitors, falsifiers, valuation_method, valuation_bear, valuation_base, \
              valuation_bull, valuation_current_price, profile_md, turn_count, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, 1, now()) \
             ON CONFLICT (user_id, ticker) DO UPDATE SET \
               company_name = COALESCE($3, company_profiles.company_name), \
               thesis = COALESCE($4, company_profiles.thesis), \
               research_status = COALESCE($5, company_profiles.research_status), \
               confidence = COALESCE($6, company_profiles.confidence), \
               bull = COALESCE($7, company_profiles.bull), \
               bear = COALESCE($8, company_profiles.bear), \
               monitors = COALESCE($9, company_profiles.monitors), \
               falsifiers = COALESCE($10, company_profiles.falsifiers), \
               valuation_method = COALESCE($11, company_profiles.valuation_method), \
               valuation_bear = COALESCE($12, company_profiles.valuation_bear), \
               valuation_base = COALESCE($13, company_profiles.valuation_base), \
               valuation_bull = COALESCE($14, company_profiles.valuation_bull), \
               valuation_current_price = COALESCE($15, company_profiles.valuation_current_price), \
               profile_md = COALESCE($16, company_profiles.profile_md), \
               turn_count = company_profiles.turn_count + 1, \
               updated_at = now() \
             RETURNING ticker, company_name, thesis, research_status, confidence, bull, bear, \
                       monitors, falsifiers, valuation_method, valuation_bear, valuation_base, \
                       valuation_bull, valuation_current_price, profile_md, turn_count, \
                       created_at, updated_at",
        )
        .bind(user_id)
        .bind(&ticker)
        .bind(&patch.company_name)
        .bind(&patch.thesis)
        .bind(&patch.research_status)
        .bind(&patch.confidence)
        .bind(&patch.bull)
        .bind(&patch.bear)
        .bind(&patch.monitors)
        .bind(&patch.falsifiers)
        .bind(&patch.valuation_method)
        .bind(patch.valuation_bear)
        .bind(patch.valuation_base)
        .bind(patch.valuation_bull)
        .bind(patch.valuation_current_price)
        .bind(&patch.profile_md)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(row)
    }

    pub async fn delete(&self, user_id: &str, ticker: &str) -> Result<bool> {
        let mut tx = with_tenant(self.pool, user_id).await?;
        let result = sqlx::query("DELETE FROM company_profiles WHERE user_id = $1 AND ticker = $2")
            .bind(user_id)
            .bind(normalize_ticker(ticker))
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(result.rows_affected() == 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AuthRepository, NewUser};
    use rust_decimal_macros::dec;

    async fn user(pool: &Pool, id: &str) {
        AuthRepository::new(pool)
            .create_user(&NewUser {
                id: id.into(),
                username: format!("{id}@example.com"),
                pass_hash: "test-only".into(),
                display_name: None,
                role: "member".into(),
            })
            .await
            .expect("create user");
    }

    #[tokio::test]
    #[ignore = "需要隔离 DATABASE_URL；验证核心 workspace 仓储 RLS 与删除终局"]
    async fn live_workspace_repositories_enforce_tenant_and_deletion() {
        let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let pool = crate::connect(&database_url, 3).await.expect("connect");
        if std::env::var("ECHO_SKIP_TEST_MIGRATE").ok().as_deref() != Some("1") {
            crate::migrate(&pool).await.expect("migrate");
        }
        user(&pool, "user_a").await;
        user(&pool, "user_b").await;

        let watch = WatchlistRepository::new(&pool);
        assert!(
            watch
                .set("user_a", "aapl", Some("Apple"), "add")
                .await
                .expect("watch")
        );
        assert_eq!(watch.list("user_a").await.expect("a watch").len(), 1);
        assert!(watch.list("user_b").await.expect("b watch").is_empty());

        let portfolio = PortfolioRepository::new(&pool);
        portfolio
            .upsert(
                "user_a",
                "AAPL",
                &PortfolioUpsert {
                    company_name: Some("Apple".into()),
                    shares: Some(dec!(12.5)),
                    avg_cost: Some(dec!(180.25)),
                    ..Default::default()
                },
            )
            .await
            .expect("upsert position");
        assert_eq!(
            portfolio.list("user_a").await.expect("a positions").len(),
            1
        );
        assert!(
            portfolio
                .list("user_b")
                .await
                .expect("b positions")
                .is_empty()
        );
        assert!(portfolio.delete("user_a", "AAPL").await.expect("delete"));
        assert!(
            portfolio
                .list("user_a")
                .await
                .expect("refresh after delete")
                .is_empty(),
            "删除后强制重读不得复活"
        );

        let preferences = PreferencesRepository::new(&pool);
        let updated = preferences
            .update(
                "user_a",
                &PreferencesPatch {
                    notify_digest: Some(false),
                    ..Default::default()
                },
            )
            .await
            .expect("preferences");
        assert!(!updated.notify_digest);
        assert!(
            preferences
                .get("user_b")
                .await
                .expect("b defaults")
                .notify_digest
        );

        let research = ResearchSessionRepository::new(&pool);
        let id = research
            .save(
                "user_a",
                &SaveResearchSession {
                    ticker: "AAPL".into(),
                    question: Some("利润质量如何？".into()),
                    ..Default::default()
                },
            )
            .await
            .expect("save research");
        assert!(
            research
                .get("user_b", &id)
                .await
                .expect("isolated")
                .is_none()
        );
        assert!(
            research
                .delete("user_a", &id)
                .await
                .expect("delete research")
        );
        assert!(
            research
                .get("user_a", &id)
                .await
                .expect("refresh research")
                .is_none(),
            "研究会话删除后不得复活"
        );

        let profiles = CompanyProfileRepository::new(&pool);
        let created = profiles
            .upsert(
                "user_a",
                "AAPL",
                &CompanyProfileUpsert {
                    company_name: Some("Apple".into()),
                    thesis: Some("生态壁垒 + 服务收入占比提升".into()),
                    bull: Some(vec!["生态锁定".into()]),
                    ..Default::default()
                },
            )
            .await
            .expect("create profile");
        assert_eq!(created.turn_count, 1);
        assert_eq!(created.bull, Some(vec!["生态锁定".to_string()]));
        assert!(
            profiles
                .get("user_b", "AAPL")
                .await
                .expect("b profile")
                .is_none(),
            "档案必须按租户隔离"
        );
        let updated = profiles
            .upsert(
                "user_a",
                "AAPL",
                &CompanyProfileUpsert {
                    research_status: Some("tracking".into()),
                    ..Default::default()
                },
            )
            .await
            .expect("update profile");
        assert_eq!(updated.turn_count, 2, "每次 upsert 都应推进 turn_count");
        assert_eq!(
            updated.thesis,
            Some("生态壁垒 + 服务收入占比提升".to_string()),
            "未传的字段必须保留原值，不是被清空"
        );
        assert_eq!(profiles.list("user_a", 20).await.expect("list").len(), 1);
        assert!(
            profiles
                .delete("user_a", "AAPL")
                .await
                .expect("delete profile")
        );
        assert!(
            profiles
                .get("user_a", "AAPL")
                .await
                .expect("refresh profile")
                .is_none(),
            "档案删除后不得复活"
        );
    }
}
