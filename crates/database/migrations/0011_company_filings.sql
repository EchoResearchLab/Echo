-- 公司公告/披露读模型（Finnhub SEC filings index）。结构化元数据（form/日期/来源链接），
-- 不含金额——filings 是"哪份文件、何时、去哪读"，不是财务数字，因此无需 currency 字段。
CREATE TABLE IF NOT EXISTS "company_filings" (
  "id" bigserial PRIMARY KEY,
  "ticker" text NOT NULL REFERENCES "companies" ("ticker"),
  "form" text NOT NULL,
  "filed_date" date,
  "accepted_date" timestamptz,
  "report_url" text,
  "filing_url" text NOT NULL,
  "source" text NOT NULL DEFAULT 'finnhub',
  "valid_time" timestamptz NOT NULL,
  "knowledge_time" timestamptz NOT NULL DEFAULT now(),
  CONSTRAINT "company_filings_ticker_filing_url_unique" UNIQUE ("ticker", "filing_url")
);

CREATE INDEX IF NOT EXISTS "idx_company_filings_ticker_filed_date"
  ON "company_filings" ("ticker", "filed_date" DESC);
