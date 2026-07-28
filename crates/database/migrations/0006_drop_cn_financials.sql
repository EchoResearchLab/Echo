-- A 股退场收缩迁移（docs/PLAN.md v3 P3 第一项的第二步，破坏性，已获负责人单独批准）。
-- 下线 PR (#37) 已删除全部读写方——cn_financials / cn_filing_ingest_log 自那之后
-- 是零调用的死表；表内 119 + 67 行为旧 A 股管道采集的数据，DROP 前已用 pg_dump
-- 备份（cn_financials_backup_2026-07-15.sql，保存在仓库外）。
-- 用户持仓/看盘里的存量 .SS/.SZ 条目不受影响：它们存在 portfolio_positions /
-- watchlist_prefs，本迁移只收缩财报仓库表。

DROP TABLE IF EXISTS "cn_filing_ingest_log";
DROP TABLE IF EXISTS "cn_financials";
