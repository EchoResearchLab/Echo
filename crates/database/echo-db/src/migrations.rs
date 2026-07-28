//! PostgreSQL 迁移执行器。
//!
//! 保持生产库 `echo_schema_migrations(name, checksum)` 的历史协议，避免重复执行已落地 SQL。
//! 迁移正文编译进二进制，部署不依赖源码目录；校验和统一使用 SHA-256 hex。

use crate::{DbError, Pool, Result};
use sha2::{Digest, Sha256};
use sqlx::{Connection, PgConnection};
use std::fmt::Write;

#[derive(Clone, Copy, Debug)]
pub struct Migration {
    pub name: &'static str,
    pub sql: &'static str,
}

const MIGRATIONS: &[Migration] = &[
    Migration {
        name: "0001_init.sql",
        sql: include_str!("../../migrations/0001_init.sql"),
    },
    Migration {
        name: "0002_tenant_rls.sql",
        sql: include_str!("../../migrations/0002_tenant_rls.sql"),
    },
    Migration {
        name: "0003_auth_session_boundary.sql",
        sql: include_str!("../../migrations/0003_auth_session_boundary.sql"),
    },
    Migration {
        name: "0004_rate_limit_buckets.sql",
        sql: include_str!("../../migrations/0004_rate_limit_buckets.sql"),
    },
    Migration {
        name: "0005_research_snapshots_daily_unique.sql",
        sql: include_str!("../../migrations/0005_research_snapshots_daily_unique.sql"),
    },
    Migration {
        name: "0006_drop_cn_financials.sql",
        sql: include_str!("../../migrations/0006_drop_cn_financials.sql"),
    },
    Migration {
        name: "0007_add_fcf_column.sql",
        sql: include_str!("../../migrations/0007_add_fcf_column.sql"),
    },
    Migration {
        name: "0008_p4_enhancements.sql",
        sql: include_str!("../../migrations/0008_p4_enhancements.sql"),
    },
    Migration {
        name: "0009_p5_team_audit_billing.sql",
        sql: include_str!("../../migrations/0009_p5_team_audit_billing.sql"),
    },
    Migration {
        name: "0010_drop_legacy_instruments.sql",
        sql: include_str!("../../migrations/0010_drop_legacy_instruments.sql"),
    },
    Migration {
        name: "0011_company_filings.sql",
        sql: include_str!("../../migrations/0011_company_filings.sql"),
    },
    Migration {
        name: "0012_scheduler_lease.sql",
        sql: include_str!("../../migrations/0012_scheduler_lease.sql"),
    },
    Migration {
        name: "0013_hk_financial_units.sql",
        sql: include_str!("../../migrations/0013_hk_financial_units.sql"),
    },
];

#[must_use]
pub const fn migrations() -> &'static [Migration] {
    MIGRATIONS
}

#[must_use]
pub fn migration_checksum(sql: &str) -> String {
    let digest = Sha256::digest(sql.as_bytes());
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

async fn migrate_locked(connection: &mut PgConnection) -> Result<Vec<&'static str>> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS echo_schema_migrations (\
            name text PRIMARY KEY, \
            checksum text NOT NULL, \
            applied_at timestamptz NOT NULL DEFAULT now()\
        )",
    )
    .execute(&mut *connection)
    .await?;

    let mut applied = Vec::new();
    for migration in MIGRATIONS {
        let checksum = migration_checksum(migration.sql);
        let existing = sqlx::query_scalar::<_, String>(
            "SELECT checksum FROM echo_schema_migrations WHERE name = $1",
        )
        .bind(migration.name)
        .fetch_optional(&mut *connection)
        .await?;
        if let Some(existing) = existing {
            if existing != checksum {
                return Err(DbError::ChangedMigration(migration.name.to_string()));
            }
            continue;
        }

        let mut tx = connection.begin().await?;
        sqlx::raw_sql(migration.sql).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO echo_schema_migrations (name, checksum) VALUES ($1, $2)")
            .bind(migration.name)
            .bind(checksum)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        applied.push(migration.name);
    }
    Ok(applied)
}

/// 串行执行所有未应用迁移，返回本次新应用的文件名。
pub async fn migrate(pool: &Pool) -> Result<Vec<&'static str>> {
    let mut connection = pool.acquire().await?;
    sqlx::query("SELECT pg_advisory_lock(hashtext('echo_schema_migrations'))")
        .execute(&mut *connection)
        .await?;
    let result = migrate_locked(&mut connection).await;
    let unlock = sqlx::query("SELECT pg_advisory_unlock(hashtext('echo_schema_migrations'))")
        .execute(&mut *connection)
        .await;
    match (result, unlock) {
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error.into()),
        (Ok(applied), Ok(_)) => Ok(applied),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_registry_is_sorted_and_unique() {
        assert_eq!(MIGRATIONS.len(), 13);
        for pair in MIGRATIONS.windows(2) {
            assert!(pair[0].name < pair[1].name, "迁移必须严格按文件名排序");
        }
    }

    #[test]
    fn checksums_are_sha256_hex_and_unique() {
        let mut checksums = MIGRATIONS
            .iter()
            .map(|migration| migration_checksum(migration.sql))
            .collect::<Vec<_>>();
        assert!(checksums.iter().all(|checksum| checksum.len() == 64));
        checksums.sort_unstable();
        checksums.dedup();
        assert_eq!(checksums.len(), MIGRATIONS.len());
    }
}
