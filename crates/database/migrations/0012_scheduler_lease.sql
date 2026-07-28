-- worker-lease（IMPROVEMENT_PLAN §4 P4-1）：多 worker 实例并发 tick 时，同一到期作业不该被
-- 两个进程同时派发。加租约字段，dispatch 前原子抢占，record_run 同时释放。
ALTER TABLE "scheduler_state"
  ADD COLUMN IF NOT EXISTS "locked_until" timestamptz,
  ADD COLUMN IF NOT EXISTS "locked_by" text;
