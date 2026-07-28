//! 调度注册表与到期判定。
//!
//! 8 个后台作业按 cron 触发。**到期判定是可恢复的纯函数**：给定「上次运行时刻」与「现在」，
//! 用 cron 算出上次之后的下一触发点，若已 ≤ 现在即为到期。worker 重启后从 `scheduler_state`
//! 拉回 last_run 重建这张表，错过的作业照样补跑。
//!
//! 作业活动体由 `activities` 模块执行，并在每次运行后记录结果。

use chrono::{DateTime, Utc};
use std::str::FromStr;

/// 一类后台作业。派发时按此分流到具体活动。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    PremarketDigest,
    AfterhoursDigest,
    MarketRefresh,
    PortfolioSnapshot,
    FalsifierCheck,
    EarningsReview,
    PositionAlert,
    ReviewReminder,
}

/// 一条调度：稳定 id（= `scheduler_state.job_id`）、cron 表达式、作业类别。
#[derive(Debug, Clone, Copy)]
pub struct Schedule {
    pub id: &'static str,
    pub cron: &'static str,
    pub job: JobKind,
}

/// 8 条调度（cron 为标准 5 段：分 时 日 月 周）。
pub const SCHEDULES: &[Schedule] = &[
    Schedule {
        id: "echo-premarket-digest",
        cron: "0 0 * * 1-5",
        job: JobKind::PremarketDigest,
    },
    Schedule {
        id: "echo-afterhours-digest",
        cron: "0 10 * * 1-5",
        job: JobKind::AfterhoursDigest,
    },
    Schedule {
        id: "echo-market-refresh",
        cron: "*/15 * * * 1-5",
        job: JobKind::MarketRefresh,
    },
    Schedule {
        id: "echo-portfolio-snapshot",
        cron: "0 22 * * 1-5",
        job: JobKind::PortfolioSnapshot,
    },
    Schedule {
        id: "echo-falsifier-check",
        cron: "*/15 * * * 1-5",
        job: JobKind::FalsifierCheck,
    },
    Schedule {
        id: "echo-earnings-review",
        cron: "30 11 * * 1-5",
        job: JobKind::EarningsReview,
    },
    Schedule {
        id: "echo-position-alert",
        cron: "*/15 * * * 1-5",
        job: JobKind::PositionAlert,
    },
    Schedule {
        id: "echo-review-reminder",
        cron: "0 9 * * 1",
        job: JobKind::ReviewReminder,
    },
];

/// cron 是否在 `(last_run, now]` 内触发过。`cron` crate 的表达式带秒字段，故给 5 段表达式补个 "0 "
/// 秒位。解析失败视为「从不触发」（返回 false）——宁可不跑，不误派。
///
/// 无 `last_run`（作业从未跑过）时以「现在」为基准判断当下这一刻是否恰为触发点，避免首启即把
/// 历史全部补跑一遍（首次注册不回填）。
#[must_use]
pub fn is_due(cron: &str, last_run: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    let with_seconds = format!("0 {cron}");
    let Ok(schedule) = cron::Schedule::from_str(&with_seconds) else {
        return false;
    };
    let after = last_run.unwrap_or(now);
    match schedule.after(&after).next() {
        Some(next) => next <= now,
        None => false,
    }
}

/// 从注册表挑出此刻到期的作业。`last_runs` 是 (job_id → 上次运行时刻) 查询函数（由 worker 从
/// `scheduler_state` 构造）。纯函数：时间与状态都从参数进，便于确定性单测。
#[must_use]
pub fn due_now<'a, F>(now: DateTime<Utc>, last_run: F) -> Vec<&'a Schedule>
where
    F: Fn(&str) -> Option<DateTime<Utc>>,
{
    SCHEDULES
        .iter()
        .filter(|s| is_due(s.cron, last_run(s.id), now))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    #[test]
    fn registry_has_eight_stable_ids() {
        assert_eq!(SCHEDULES.len(), 8);
        // id 唯一——scheduler_state.job_id 是主键，重复会 upsert 相互覆盖。
        let mut ids: Vec<&str> = SCHEDULES.iter().map(|s| s.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 8);
    }

    #[test]
    fn fires_when_interval_boundary_crossed_since_last_run() {
        // 每 15 分钟（工作日）。上次 10:00 跑过，现在 10:16：区间内跨过 10:15，到期。
        let last = utc(2026, 7, 20, 10, 0); // 周一
        let now = utc(2026, 7, 20, 10, 16);
        assert!(is_due("*/15 * * * 1-5", Some(last), now));
    }

    #[test]
    fn not_due_when_no_boundary_since_last_run() {
        // 上次 10:16 跑过，现在 10:20：下一个 15 分点是 10:30，未到。
        let last = utc(2026, 7, 20, 10, 16);
        let now = utc(2026, 7, 20, 10, 20);
        assert!(!is_due("*/15 * * * 1-5", Some(last), now));
    }

    #[test]
    fn weekend_schedule_does_not_fire_on_saturday() {
        // 工作日 premarket digest（0 0 * * 1-5），周六 00:05 不该触发。
        let sat_last = utc(2026, 7, 25, 0, 0); // 周六
        let sat_now = utc(2026, 7, 25, 0, 5);
        assert!(!is_due("0 0 * * 1-5", Some(sat_last), sat_now));
    }

    #[test]
    fn first_run_without_history_does_not_backfill() {
        // 从未跑过：现在不是触发点（10:07 非 15 分整点），不因「没历史」就补跑。
        let now = utc(2026, 7, 20, 10, 7);
        assert!(!is_due("*/15 * * * 1-5", None, now));
    }

    #[test]
    fn bad_cron_is_never_due() {
        assert!(!is_due("not a cron", None, utc(2026, 7, 20, 10, 0)));
    }

    #[test]
    fn due_now_selects_only_ready_jobs() {
        // 周一 10:30：15 分钟类作业到期（上次 10:14）；earnings-review(30 11) 未到。
        let now = utc(2026, 7, 20, 10, 30);
        let due = due_now(now, |id| match id {
            "echo-market-refresh" | "echo-falsifier-check" | "echo-position-alert" => {
                Some(utc(2026, 7, 20, 10, 14))
            }
            _ => Some(utc(2026, 7, 20, 10, 29)),
        });
        let ids: Vec<&str> = due.iter().map(|s| s.id).collect();
        assert!(ids.contains(&"echo-market-refresh"));
        assert!(ids.contains(&"echo-falsifier-check"));
        assert!(!ids.contains(&"echo-earnings-review"));
    }
}
