//! 数字护栏的质量审计底账。
//!
//! 每轮研究的护栏结论（核了几个数、pass/soft/hard 各几个、硬失败具体是哪些数字）
//! 写进 `fact_guard_audit`。没有这张底账，护栏就只是一次性的当轮提示——改了提示词
//! 或换了取数源之后，谁也说不清硬失败率是升了还是降了。
//!
//! **这张表不分租户**：它记的是模型输出质量，不是用户私有数据，所以只登记 ticker 与
//! 计数、不写 user_id，也不进 RLS。硬失败明细里同样只放答案原文里的数字与判定原因。

use crate::{Pool, Result};

/// 一个被判 hard 的数字——落进 `hard_details` jsonb。
#[derive(Clone, Debug, serde::Serialize)]
pub struct FactGuardHardDetail {
    /// 答案正文里的原文片段（如 "3.92 万亿"）。
    pub raw: String,
    /// 维度：amount / percent / multiple / date。
    pub dimension: String,
    /// 升 hard 的原因（符号相反、数量级差过大、日期查无…）。
    pub reason: Option<String>,
}

/// 一轮护栏的完整审计记录。
#[derive(Clone, Debug)]
pub struct FactGuardAuditEntry {
    pub ticker: Option<String>,
    /// 产出这段答案的链路：`ask` / `ask_stream` / `compare` / `report`。
    /// 不同链路的提示词与取数深度不同，混在一起统计会看不出是哪条腿在退化。
    pub mode: String,
    pub total: i32,
    pub pass_count: i32,
    pub soft_count: i32,
    pub hard_count: i32,
    pub hard_details: Vec<FactGuardHardDetail>,
}

pub struct FactGuardAuditRepository<'a> {
    pool: &'a Pool,
}

impl<'a> FactGuardAuditRepository<'a> {
    #[must_use]
    pub fn new(pool: &'a Pool) -> Self {
        Self { pool }
    }

    /// 追加一条护栏审计。
    ///
    /// `hard_details` 为空时写 `NULL` 而不是 `[]`——"这轮没有硬失败"和"这轮没记明细"
    /// 在统计上是两回事，空数组会让后者伪装成前者。
    pub async fn record(&self, entry: &FactGuardAuditEntry) -> Result<()> {
        let hard_details = if entry.hard_details.is_empty() {
            None
        } else {
            Some(serde_json::to_value(&entry.hard_details).unwrap_or(serde_json::Value::Null))
        };
        sqlx::query(
            "INSERT INTO fact_guard_audit \
             (ticker, mode, total, pass_count, soft_count, hard_count, hard_details, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, now())",
        )
        .bind(entry.ticker.as_deref())
        .bind(&entry.mode)
        .bind(entry.total)
        .bind(entry.pass_count)
        .bind(entry.soft_count)
        .bind(entry.hard_count)
        .bind(hard_details)
        .execute(self.pool)
        .await?;
        Ok(())
    }
}
