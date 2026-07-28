//! 后台工作流 worker——Rust cron 调度与可恢复活动执行器。
//!
//! 承接两类既有巡检：业绩复盘（刷新 earnings_calendar 已报告字段）与证伪巡检（对 watch_rules
//! 逐条核对基本面/价格线），外加行情刷新/组合快照/摘要推送等共 8 个 cron 作业。
//!
//! 每分钟一跳，从 `scheduler_state` 拉回各作业 last_run 重建到期表，
//! 用 [`schedule::due_now`] 纯函数挑出到期作业，派发后把运行结果 upsert 回 `scheduler_state`——
//! 重启后错过的作业照样补跑。活动执行结果写回调度状态，失败会保留错误信息供下一轮重试。

mod activities;
mod schedule;

use std::collections::HashMap;
use std::time::Duration;

use activities::Activities;
use echo_config::WorkerConfig;
use echo_db::{Pool, SchedulerStateRepository};
use schedule::{Schedule, due_now};
use tracing::{debug, error, info};

/// 租约时长：留足 worker 最慢活动的余量，同时给崩溃恢复设个上限——持锁进程若中途失联，
/// 下一跳最多等这么久就能重新抢到，不会永久卡死一个作业。
const LEASE_SECONDS: i64 = 900;

/// 从 `scheduler_state` 拉回 (job_id → last_run_at)，重建到期判定所需的历史表。
async fn load_last_runs(pool: &Pool) -> HashMap<String, chrono::DateTime<chrono::Utc>> {
    match SchedulerStateRepository::new(pool).all().await {
        Ok(rows) => rows
            .into_iter()
            .filter_map(|r| r.last_run_at.map(|t| (r.job_id, t)))
            .collect(),
        Err(err) => {
            error!(error = %err, "读取 scheduler_state 失败，本跳按空历史处理");
            HashMap::new()
        }
    }
}

/// 派发一个到期作业前先抢占租约（worker-lease）：多实例部署下，
/// 同一 job_id 在同一时刻只有一个实例能抢到，抢不到就说明另一实例正在跑或刚跑完这一跳，
/// 本实例安静跳过，不重复执行。实际成功才记 `ok`；失败详情进入可恢复游标供运营排查。
// tracing span 是 OTLP 追踪导出的唯一数据来源（见 echo-observability），没有它配了
// OTLP 端点也无 span 可导——同 echo-api 的 TraceLayer 是同一条"新增导出通路必须同 PR
// 接上调用方"教训。job_id/worker_id 进 span 字段，方便按作业类型/实例过滤追踪。
#[tracing::instrument(skip(pool, activities), fields(job_id = schedule.id, worker_id))]
async fn dispatch(pool: &Pool, activities: &Activities, schedule: &Schedule, worker_id: &str) {
    match SchedulerStateRepository::new(pool)
        .try_claim(schedule.id, worker_id, LEASE_SECONDS)
        .await
    {
        Ok(true) => {}
        Ok(false) => {
            debug!(job_id = schedule.id, "租约被其他 worker 实例持有，本跳跳过");
            return;
        }
        Err(err) => {
            error!(job_id = schedule.id, error = %err, "抢占作业租约失败，本跳跳过");
            return;
        }
    }
    let (status, detail) = match activities.run(schedule.job).await {
        Ok(detail) => {
            info!(job_id = schedule.id, detail, "后台活动完成");
            ("ok", detail)
        }
        Err(err) => {
            error!(job_id = schedule.id, error = %err, "后台活动失败");
            ("error", err.to_string())
        }
    };
    if let Err(err) = SchedulerStateRepository::new(pool)
        .record_run(schedule.id, status, Some(&detail))
        .await
    {
        error!(job_id = schedule.id, error = %err, "记录运行状态失败");
    }
}

/// 跑一跳：重建历史 → 挑到期 → 逐个派发。抽出来便于将来做单跳集成测试。
async fn tick(
    pool: &Pool,
    activities: &Activities,
    now: chrono::DateTime<chrono::Utc>,
    worker_id: &str,
) {
    let last_runs = load_last_runs(pool).await;
    let due = due_now(now, |id| last_runs.get(id).copied());
    for schedule in due {
        dispatch(pool, activities, schedule, worker_id).await;
    }
}

/// 本进程的租约持有者标识——进程号 + 启动时纳秒时间戳，足够在单机多实例/多次重启间区分，
/// 不需要为此单独引入 uuid 依赖。
fn worker_identity() -> String {
    format!(
        "pid{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    )
}

#[tokio::main]
async fn main() {
    echo_observability::init("echo-worker").expect("init tracing");
    let config = WorkerConfig::from_env().expect("load echo-worker config");
    // 调度必须有可恢复的状态底座：没配 DATABASE_URL 就硬失败——宁可启动即报，也不跑一个状态不落库、
    // 重启即丢的假调度器（那正是「写好了没人调」类隐形失效）。
    let pool = echo_db::connect(&config.database_url, config.max_connections)
        .await
        .expect("connect DATABASE_URL");
    let state = SchedulerStateRepository::new(&pool);
    for job in schedule::SCHEDULES {
        state.register_job(job.id).await.expect("register cron job");
    }
    let activities = Activities::new(pool.clone(), config.data_sources, config.email)
        .await
        .expect("build worker activities");
    let worker_id = worker_identity();
    info!(
        jobs = schedule::SCHEDULES.len(),
        tick_seconds = config.tick_seconds,
        worker_id,
        "echo-worker started; 8 个活动均由 Rust 执行，dispatch 前抢占租约防多实例重复执行"
    );

    let mut ticker = tokio::time::interval(Duration::from_secs(config.tick_seconds));
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                tick(&pool, &activities, chrono::Utc::now(), &worker_id).await;
            }
            _ = shutdown_signal() => {
                info!("echo-worker 收到停机信号，当前无进行中活动，安全退出");
                break;
            }
        }
    }
    echo_observability::shutdown();
}

/// 与 echo-api 同一套信号约定：SIGTERM/Ctrl+C。`select!` 只在空闲等待下一跳时参与选择，
/// 一旦某个 `tick()` 已经开始执行就会跑到完成（不会被同一次 select 打断），避免在活动
/// 持有 lease 的中途被杀死导致租约要等到过期才能被下一个实例接手。
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("install ctrl+c handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
}
