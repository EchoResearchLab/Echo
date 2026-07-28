-- research_snapshots 的判断粒度是"某天我们怎么看这家公司"，不是"某一轮对话"。
-- 没有这条唯一约束时，每轮研究都会插一条新快照：同一天的几十条近乎相同的判断会
-- 同时满 14 天成熟期、同时进 computeTickerScorecard 的分母，等于让聊得最多的那天
-- 决定整只票的命中率。改为一天一条，当天复研究覆盖当天那条。

-- 先合并历史重复：保留每 (user_id, ticker, valid_time) 最新写入（knowledge_time 最大，
-- 并列时取 id 最大）的那条，其余删除，否则唯一索引建不起来。
DELETE FROM "research_snapshots" a
USING "research_snapshots" b
WHERE a."user_id" = b."user_id"
  AND a."ticker" = b."ticker"
  AND a."valid_time" = b."valid_time"
  AND (a."knowledge_time", a."id") < (b."knowledge_time", b."id");

CREATE UNIQUE INDEX IF NOT EXISTS "uq_research_snapshots_user_ticker_day"
  ON "research_snapshots" ("user_id", "ticker", "valid_time");
