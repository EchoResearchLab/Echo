use crate::{Pool, Result};

/// 共享限流桶（`rate_limit_buckets`）——跨 API 副本对同一 key 计数，窗口过期即重置。
/// 单条 UPSERT 原子完成"读计数 + 判断窗口 + 递增/重置"，无需应用层加锁。
pub struct RateLimitRepository<'a> {
    pool: &'a Pool,
}

impl<'a> RateLimitRepository<'a> {
    #[must_use]
    pub fn new(pool: &'a Pool) -> Self {
        Self { pool }
    }

    /// 尝试消费一次配额；`true` 表示本次在限额内放行，`false` 表示已超限。
    pub async fn try_consume(&self, key: &str, limit: i32, window_seconds: i64) -> Result<bool> {
        let (count,): (i32,) = sqlx::query_as(
            "INSERT INTO rate_limit_buckets (key, count, reset_at) \
             VALUES ($1, 1, now() + make_interval(secs => $2)) \
             ON CONFLICT (key) DO UPDATE SET \
               count = CASE WHEN rate_limit_buckets.reset_at <= now() \
                            THEN 1 ELSE rate_limit_buckets.count + 1 END, \
               reset_at = CASE WHEN rate_limit_buckets.reset_at <= now() \
                               THEN now() + make_interval(secs => $2) \
                               ELSE rate_limit_buckets.reset_at END \
             RETURNING count",
        )
        .bind(key)
        .bind(window_seconds as f64)
        .fetch_one(self.pool)
        .await?;
        Ok(count <= limit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "需要隔离 DATABASE_URL；验证限流桶窗口内计数与超限拒绝"]
    async fn live_rate_limit_allows_within_window_then_rejects() {
        let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let pool = crate::connect(&database_url, 3).await.expect("connect");
        if std::env::var("ECHO_SKIP_TEST_MIGRATE").ok().as_deref() != Some("1") {
            crate::migrate(&pool).await.expect("migrate");
        }
        let key = format!("test:{}", uuid_like());
        let repo = RateLimitRepository::new(&pool);
        assert!(repo.try_consume(&key, 2, 60).await.expect("first"));
        assert!(repo.try_consume(&key, 2, 60).await.expect("second"));
        assert!(!repo.try_consume(&key, 2, 60).await.expect("third exceeds"));
    }

    fn uuid_like() -> String {
        format!(
            "{:x}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        )
    }
}
