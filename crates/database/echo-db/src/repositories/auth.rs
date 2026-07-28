use crate::{Pool, Result, with_tenant};
use chrono::{DateTime, Utc};
use sqlx::{FromRow, Postgres};

#[derive(Clone, Debug, FromRow)]
pub struct UserRow {
    pub id: String,
    pub username: String,
    pub pass_hash: String,
    pub display_name: Option<String>,
    pub role: String,
    pub created_at: DateTime<Utc>,
    pub last_login_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug)]
pub struct NewUser {
    pub id: String,
    pub username: String,
    pub pass_hash: String,
    pub display_name: Option<String>,
    pub role: String,
}

#[derive(Clone, Debug, FromRow)]
pub struct AuthSessionRow {
    pub token_hash: String,
    pub user_id: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Clone, Debug, FromRow)]
pub struct AccountSubscriptionRow {
    pub plan_id: String,
    pub plan_name: String,
    pub tier: String,
    pub status: String,
    pub current_period_end: DateTime<Utc>,
    pub max_daily_calls: i32,
    pub features: Vec<String>,
}

pub struct AuthRepository<'a> {
    pool: &'a Pool,
}

impl<'a> AuthRepository<'a> {
    #[must_use]
    pub fn new(pool: &'a Pool) -> Self {
        Self { pool }
    }

    pub async fn user_by_id(&self, id: &str) -> Result<Option<UserRow>> {
        self.user_by("id", id).await
    }

    pub async fn user_by_username(&self, username: &str) -> Result<Option<UserRow>> {
        self.user_by("username", &username.trim().to_lowercase())
            .await
    }

    async fn user_by(&self, column: &str, value: &str) -> Result<Option<UserRow>> {
        // column 只来自上面两个固定调用，不接外部输入。
        let query = format!(
            "SELECT id, username, pass_hash, display_name, role, created_at, last_login_at \
             FROM users WHERE {column} = $1 LIMIT 1"
        );
        Ok(sqlx::query_as::<_, UserRow>(&query)
            .bind(value)
            .fetch_optional(self.pool)
            .await?)
    }

    pub async fn user_count(&self) -> Result<i64> {
        Ok(sqlx::query_scalar("SELECT count(*) FROM users")
            .fetch_one(self.pool)
            .await?)
    }

    pub async fn create_user(&self, input: &NewUser) -> Result<UserRow> {
        Ok(insert_user(self.pool, input).await?)
    }

    pub async fn ensure_local_user(&self, id: &str) -> Result<UserRow> {
        sqlx::query(
            "INSERT INTO users (id, username, pass_hash, display_name, role) \
             VALUES ($1, lower($1), '!local-auth-disabled', '本机用户', 'owner') \
             ON CONFLICT (id) DO NOTHING",
        )
        .bind(id)
        .execute(self.pool)
        .await?;
        self.user_by_id(id)
            .await?
            .ok_or_else(|| sqlx::Error::RowNotFound.into())
    }

    pub async fn touch_last_login(&self, user_id: &str) -> Result<()> {
        sqlx::query("UPDATE users SET last_login_at = now() WHERE id = $1")
            .bind(user_id)
            .execute(self.pool)
            .await?;
        Ok(())
    }

    pub async fn subscription_for_user(
        &self,
        user_id: &str,
    ) -> Result<Option<AccountSubscriptionRow>> {
        Ok(sqlx::query_as::<_, AccountSubscriptionRow>(
            "SELECT s.plan_id, p.name AS plan_name, p.tier, s.status, \
                    s.current_period_end, p.max_daily_calls, \
                    COALESCE(p.features, ARRAY[]::text[]) AS features \
             FROM subscriptions s \
             JOIN plans p ON p.id = s.plan_id \
             WHERE s.user_id = $1 \
             ORDER BY CASE WHEN s.status = 'active' THEN 0 ELSE 1 END, s.updated_at DESC \
             LIMIT 1",
        )
        .bind(user_id)
        .fetch_optional(self.pool)
        .await?)
    }

    pub async fn create_invite(
        &self,
        code: &str,
        note: Option<&str>,
        created_by: Option<&str>,
    ) -> Result<()> {
        sqlx::query("INSERT INTO invite_codes (code, note, created_by) VALUES ($1, $2, $3)")
            .bind(code)
            .bind(note)
            .bind(created_by)
            .execute(self.pool)
            .await?;
        Ok(())
    }

    pub async fn create_user_with_invite(
        &self,
        input: &NewUser,
        invite_code: &str,
    ) -> Result<UserRow> {
        let mut tx = self.pool.begin().await?;
        let invite = sqlx::query_scalar::<_, String>(
            "SELECT code FROM invite_codes WHERE code = $1 AND used_by IS NULL FOR UPDATE",
        )
        .bind(invite_code.trim())
        .fetch_optional(&mut *tx)
        .await?;
        if invite.is_none() {
            return Err(sqlx::Error::RowNotFound.into());
        }
        let user = insert_user(&mut *tx, input).await?;
        let consumed = sqlx::query(
            "UPDATE invite_codes SET used_by = $1, used_at = now() \
             WHERE code = $2 AND used_by IS NULL",
        )
        .bind(&input.id)
        .bind(invite_code.trim())
        .execute(&mut *tx)
        .await?;
        if consumed.rows_affected() != 1 {
            return Err(sqlx::Error::RowNotFound.into());
        }
        tx.commit().await?;
        Ok(user)
    }

    pub async fn insert_session(
        &self,
        token_hash: &str,
        user_id: &str,
        expires_at: DateTime<Utc>,
    ) -> Result<()> {
        let mut tx = with_tenant(self.pool, user_id).await?;
        sqlx::query(
            "INSERT INTO auth_sessions (token_hash, user_id, expires_at, last_seen_at) \
             VALUES ($1, $2, $3, now())",
        )
        .bind(token_hash)
        .bind(user_id)
        .bind(expires_at)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn live_session(
        &self,
        token_hash: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<AuthSessionRow>> {
        Ok(sqlx::query_as::<_, AuthSessionRow>(
            "SELECT token_hash, user_id, expires_at FROM authenticate_session($1, $2)",
        )
        .bind(token_hash)
        .bind(now)
        .fetch_optional(self.pool)
        .await?)
    }

    pub async fn refresh_session(
        &self,
        token_hash: &str,
        expires_at: DateTime<Utc>,
    ) -> Result<bool> {
        Ok(sqlx::query_scalar("SELECT refresh_auth_session($1, $2)")
            .bind(token_hash)
            .bind(expires_at)
            .fetch_one(self.pool)
            .await?)
    }

    pub async fn delete_session(&self, token_hash: &str) -> Result<bool> {
        Ok(sqlx::query_scalar("SELECT delete_auth_session($1)")
            .bind(token_hash)
            .fetch_one(self.pool)
            .await?)
    }

    pub async fn prune_expired_sessions(&self, now: DateTime<Utc>) -> Result<i64> {
        Ok(sqlx::query_scalar("SELECT prune_auth_sessions($1)")
            .bind(now)
            .fetch_one(self.pool)
            .await?)
    }
}

async fn insert_user<'e, E>(
    executor: E,
    input: &NewUser,
) -> std::result::Result<UserRow, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = Postgres>,
{
    sqlx::query_as::<_, UserRow>(
        "INSERT INTO users (id, username, pass_hash, display_name, role) \
         VALUES ($1, lower($2), $3, $4, $5) \
         RETURNING id, username, pass_hash, display_name, role, created_at, last_login_at",
    )
    .bind(&input.id)
    .bind(input.username.trim())
    .bind(&input.pass_hash)
    .bind(&input.display_name)
    .bind(&input.role)
    .fetch_one(executor)
    .await
}
