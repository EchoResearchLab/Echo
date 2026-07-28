-- Shared abuse-rate counter for heavy endpoints (ask/report-generate/parse-document),
-- so the limit holds across API replicas without adding a new architecture component.
CREATE TABLE IF NOT EXISTS "rate_limit_buckets" (
  "key" text PRIMARY KEY,
  "count" integer NOT NULL DEFAULT 1,
  "reset_at" timestamptz NOT NULL
);

CREATE INDEX IF NOT EXISTS "idx_rate_limit_buckets_reset_at" ON "rate_limit_buckets" ("reset_at");
