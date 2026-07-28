CREATE TABLE IF NOT EXISTS "companies" (
	"ticker" text PRIMARY KEY NOT NULL,
	"name_zh" text NOT NULL,
	"name_en" text,
	"sector" text,
	"industry" text,
	"listing_status" text DEFAULT 'active' NOT NULL,
	"exchange" text DEFAULT 'HKEX' NOT NULL,
	"currency" text DEFAULT 'HKD' NOT NULL,
	"market_cap_category" text,
	"is_hsi" boolean DEFAULT false NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS "company_details" (
	"ticker" text PRIMARY KEY NOT NULL,
	"aliases" text[],
	"price" numeric,
	"market_cap" text,
	"week_52_range" text,
	"dividend_yield" text,
	"pe" text,
	"pb" text,
	"ps" text,
	"latest_report" text,
	"status" text,
	"status_tone" text,
	"summary" text[],
	"business_model" text[],
	"metrics" text[],
	"moat" text[],
	"management" text[],
	"risks" text[],
	"bull_case" text[],
	"bear_case" text[],
	"monitors" text[],
	"official_sources" text[],
	"valid_time" timestamp with time zone DEFAULT now(),
	"knowledge_time" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS "market_snapshots" (
	"id" serial PRIMARY KEY NOT NULL,
	"ticker" text NOT NULL,
	"price" numeric,
	"previous_close" numeric,
	"change" numeric,
	"change_percent" numeric,
	"open" numeric,
	"high" numeric,
	"low" numeric,
	"volume" numeric,
	"market_cap" numeric,
	"pe" numeric,
	"dividend_yield" numeric,
	"week_52_high" numeric,
	"week_52_low" numeric,
	"source" text,
	"valid_time" timestamp with time zone NOT NULL,
	"knowledge_time" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS "auth_sessions" (
	"token_hash" text PRIMARY KEY NOT NULL,
	"user_id" text NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	"expires_at" timestamp with time zone NOT NULL,
	"last_seen_at" timestamp with time zone
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS "invite_codes" (
	"code" text PRIMARY KEY NOT NULL,
	"note" text,
	"created_by" text,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	"used_by" text,
	"used_at" timestamp with time zone
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS "users" (
	"id" text PRIMARY KEY NOT NULL,
	"username" text NOT NULL,
	"pass_hash" text NOT NULL,
	"display_name" text,
	"role" text DEFAULT 'member' NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	"last_login_at" timestamp with time zone,
	CONSTRAINT "users_username_unique" UNIQUE("username")
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS "company_profiles" (
	"user_id" text DEFAULT 'local' NOT NULL,
	"ticker" text NOT NULL,
	"company_name" text,
	"thesis" text,
	"research_status" text,
	"confidence" text,
	"bull" text[],
	"bear" text[],
	"monitors" text[],
	"falsifiers" text[],
	"valuation_method" text,
	"valuation_bear" numeric,
	"valuation_base" numeric,
	"valuation_bull" numeric,
	"valuation_current_price" numeric,
	"profile_md" text,
	"turn_count" integer DEFAULT 0 NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "company_profiles_user_id_ticker_pk" PRIMARY KEY("user_id","ticker")
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS "profile_events" (
	"id" bigserial PRIMARY KEY NOT NULL,
	"user_id" text DEFAULT 'local' NOT NULL,
	"ticker" text NOT NULL,
	"date" text NOT NULL,
	"kind" text NOT NULL,
	"summary" text NOT NULL,
	"rationale" text,
	"evidence_json" jsonb,
	"session_id" text,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS "research_sessions" (
	"id" text PRIMARY KEY NOT NULL,
	"user_id" text DEFAULT 'local' NOT NULL,
	"ticker" text,
	"title" text,
	"question" text,
	"conversation_id" text,
	"status" text DEFAULT 'draft' NOT NULL,
	"report_markdown" text,
	"rating" text,
	"confidence" text,
	"decision_panel" jsonb,
	"full_research" text,
	"data_sources" jsonb,
	"thread_json" jsonb,
	"turn_count" integer,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS "research_snapshots" (
	"id" bigserial PRIMARY KEY NOT NULL,
	"user_id" text DEFAULT 'local' NOT NULL,
	"ticker" text NOT NULL,
	"valid_time" date NOT NULL,
	"thesis" text,
	"valuation_position" text,
	"valuation_bear" numeric,
	"valuation_base" numeric,
	"valuation_bull" numeric,
	"valuation_currency" text,
	"price_at_snapshot" numeric,
	"falsifiers" text[],
	"session_id" text,
	"knowledge_time" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS "portfolio_positions" (
	"user_id" text DEFAULT 'local' NOT NULL,
	"ticker" text NOT NULL,
	"company_name" text,
	"shares" numeric,
	"avg_cost" numeric,
	"stop_loss" numeric,
	"take_profit" numeric,
	"note" text,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "portfolio_positions_user_id_ticker_pk" PRIMARY KEY("user_id","ticker")
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS "portfolio_snapshot_totals" (
	"id" bigserial PRIMARY KEY NOT NULL,
	"user_id" text DEFAULT 'local' NOT NULL,
	"snapshot_valid_time" date NOT NULL,
	"currency" text NOT NULL,
	"market_value" numeric NOT NULL
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS "portfolio_snapshots" (
	"user_id" text DEFAULT 'local' NOT NULL,
	"valid_time" date NOT NULL,
	"total_value_usd" numeric,
	"total_cost_usd" numeric,
	"total_pnl_usd" numeric,
	"position_count" integer NOT NULL,
	"knowledge_time" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "portfolio_snapshots_user_id_valid_time_pk" PRIMARY KEY("user_id","valid_time")
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS "watch_rules" (
	"id" bigserial PRIMARY KEY NOT NULL,
	"user_id" text DEFAULT 'local' NOT NULL,
	"ticker" text NOT NULL,
	"kind" text NOT NULL,
	"threshold" numeric NOT NULL,
	"metric" text,
	"label" text,
	"source" text DEFAULT 'falsifier' NOT NULL,
	"session_id" text,
	"active" boolean DEFAULT true NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	"last_triggered_at" timestamp with time zone
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS "watchlist_prefs" (
	"user_id" text DEFAULT 'local' NOT NULL,
	"ticker" text NOT NULL,
	"company_name" text,
	"mode" text DEFAULT 'add' NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "watchlist_prefs_user_id_ticker_pk" PRIMARY KEY("user_id","ticker")
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS "cn_filing_ingest_log" (
	"ticker" text PRIMARY KEY NOT NULL,
	"status" text NOT NULL,
	"detail" text,
	"announcements_found" integer DEFAULT 0 NOT NULL,
	"ingested_count" integer DEFAULT 0 NOT NULL,
	"valid_time" timestamp with time zone DEFAULT now() NOT NULL,
	"knowledge_time" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS "cn_financials" (
	"id" bigserial PRIMARY KEY NOT NULL,
	"ticker" text NOT NULL,
	"period_label" text,
	"valid_time" text,
	"period_type" text,
	"currency" text,
	"unit_label" text,
	"revenue" numeric,
	"revenue_prior" numeric,
	"gross_profit" numeric,
	"gross_profit_prior" numeric,
	"operating_income" numeric,
	"operating_income_prior" numeric,
	"net_income" numeric,
	"net_income_prior" numeric,
	"net_income_attributable" numeric,
	"eps" numeric,
	"operating_cash_flow" numeric,
	"cash_and_equivalents" numeric,
	"net_cash" numeric,
	"source_title" text,
	"source_url" text,
	"published_at" timestamp with time zone,
	"knowledge_time" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "cn_financials_source_url_unique" UNIQUE("source_url")
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS "comp_peers" (
	"ticker" text PRIMARY KEY NOT NULL,
	"stage" text,
	"peers_json" jsonb,
	"anchor_json" jsonb,
	"provider_status" text DEFAULT 'missing' NOT NULL,
	"detail" text,
	"partial" boolean DEFAULT false NOT NULL,
	"valid_time" timestamp with time zone DEFAULT now() NOT NULL,
	"knowledge_time" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS "earnings_calendar" (
	"ticker" text PRIMARY KEY NOT NULL,
	"next_date" text,
	"quarter" integer,
	"year" integer,
	"eps_estimate" numeric,
	"revenue_estimate" numeric,
	"source" text,
	"provider_status" text DEFAULT 'missing' NOT NULL,
	"detail" text,
	"last_date" text,
	"last_quarter" integer,
	"last_year" integer,
	"last_eps_estimate" numeric,
	"last_eps_actual" numeric,
	"last_revenue_estimate" numeric,
	"last_revenue_actual" numeric,
	"last_eps_surprise_pct" numeric,
	"last_revenue_surprise_pct" numeric,
	"valid_time" text,
	"knowledge_time" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS "historical_valuation" (
	"ticker" text PRIMARY KEY NOT NULL,
	"provider_status" text DEFAULT 'missing' NOT NULL,
	"detail" text,
	"valid_time" timestamp with time zone,
	"knowledge_time" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS "historical_valuation_points" (
	"id" bigserial PRIMARY KEY NOT NULL,
	"ticker" text NOT NULL,
	"period_end_date" text NOT NULL,
	"pe_value" numeric
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS "hk_buybacks" (
	"id" bigserial PRIMARY KEY NOT NULL,
	"ticker" text NOT NULL,
	"trade_date" text,
	"shares_repurchased" numeric,
	"price_high" numeric,
	"price_low" numeric,
	"total_consideration" numeric,
	"currency" text,
	"shares_issued_total" numeric,
	"period_end_date" text,
	"source_title" text,
	"source_url" text,
	"published_at" timestamp with time zone,
	"knowledge_time" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "hk_buybacks_source_url_unique" UNIQUE("source_url")
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS "hk_filing_ingest_log" (
	"ticker" text PRIMARY KEY NOT NULL,
	"status" text NOT NULL,
	"detail" text,
	"announcements_found" integer DEFAULT 0 NOT NULL,
	"ingested_count" integer DEFAULT 0 NOT NULL,
	"valid_time" timestamp with time zone DEFAULT now() NOT NULL,
	"knowledge_time" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS "hk_financials" (
	"id" bigserial PRIMARY KEY NOT NULL,
	"ticker" text NOT NULL,
	"period_label" text,
	"valid_time" text,
	"period_type" text,
	"currency" text,
	"unit_label" text,
	"revenue" numeric,
	"revenue_prior" numeric,
	"gross_profit" numeric,
	"gross_profit_prior" numeric,
	"operating_income" numeric,
	"operating_income_prior" numeric,
	"net_income" numeric,
	"net_income_prior" numeric,
	"net_income_attributable" numeric,
	"eps" numeric,
	"operating_cash_flow" numeric,
	"cash_and_equivalents" numeric,
	"net_cash" numeric,
	"source_title" text,
	"source_url" text,
	"published_at" timestamp with time zone,
	"knowledge_time" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "hk_financials_source_url_unique" UNIQUE("source_url")
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS "insider_activity" (
	"ticker" text PRIMARY KEY NOT NULL,
	"provider_status" text DEFAULT 'missing' NOT NULL,
	"net_shares" numeric,
	"net_value_usd" numeric,
	"buy_count" integer,
	"sell_count" integer,
	"distinct_insiders" integer,
	"valid_time" timestamp with time zone,
	"transactions_json" jsonb,
	"detail" text,
	"knowledge_time" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS "web_evidence" (
	"id" text PRIMARY KEY NOT NULL,
	"ticker" text NOT NULL,
	"intent" text NOT NULL,
	"query" text,
	"title" text,
	"url" text NOT NULL,
	"source" text,
	"source_type" text,
	"snippet" text,
	"valid_time" timestamp with time zone,
	"knowledge_time" timestamp with time zone NOT NULL,
	"relevance_score" numeric,
	"credibility_score" numeric,
	"content_hash" text,
	"raw_json" jsonb,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS "notifications" (
	"id" bigserial PRIMARY KEY NOT NULL,
	"user_id" text DEFAULT 'local' NOT NULL,
	"kind" text NOT NULL,
	"title" text NOT NULL,
	"body" text,
	"ticker" text,
	"payload" jsonb,
	"dedupe_key" text,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	"read_at" timestamp with time zone
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS "canary_runs" (
	"id" serial PRIMARY KEY NOT NULL,
	"batch_id" text NOT NULL,
	"source" text NOT NULL,
	"ticker" text NOT NULL,
	"status" text NOT NULL,
	"detail" text,
	"latency_ms" integer,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS "documents" (
	"id" text PRIMARY KEY NOT NULL,
	"user_id" text DEFAULT 'local' NOT NULL,
	"ticker" text,
	"name" text NOT NULL,
	"mime_type" text,
	"size" integer,
	"parser" text,
	"text" text,
	"summary" text,
	"source_type" text DEFAULT 'upload' NOT NULL,
	"source_url" text,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS "fact_guard_audit" (
	"id" serial PRIMARY KEY NOT NULL,
	"ticker" text,
	"mode" text NOT NULL,
	"total" integer DEFAULT 0 NOT NULL,
	"pass_count" integer DEFAULT 0 NOT NULL,
	"soft_count" integer DEFAULT 0 NOT NULL,
	"hard_count" integer DEFAULT 0 NOT NULL,
	"hard_details" jsonb,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS "feedback" (
	"id" serial PRIMARY KEY NOT NULL,
	"user_id" text NOT NULL,
	"message" text NOT NULL,
	"context_json" jsonb,
	"status" text DEFAULT 'new' NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS "llm_audit" (
	"id" bigserial PRIMARY KEY NOT NULL,
	"user_id" text DEFAULT 'local' NOT NULL,
	"provider" text NOT NULL,
	"model" text,
	"kind" text DEFAULT 'chat' NOT NULL,
	"status" text NOT NULL,
	"latency_ms" integer,
	"error_detail" text,
	"input_tokens" integer,
	"output_tokens" integer,
	"estimated_cost_usd" numeric,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS "scheduler_state" (
	"job_id" text PRIMARY KEY NOT NULL,
	"last_run_at" timestamp with time zone,
	"last_status" text,
	"last_detail" text
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS "user_preferences" (
	"user_id" text PRIMARY KEY NOT NULL,
	"onboarding_completed" boolean DEFAULT false NOT NULL,
	"notify_digest" boolean DEFAULT true NOT NULL,
	"notify_positions" boolean DEFAULT true NOT NULL,
	"notify_falsify" boolean DEFAULT true NOT NULL,
	"notify_review" boolean DEFAULT true NOT NULL,
	"notify_earnings" boolean DEFAULT true NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
DO $$ BEGIN
 ALTER TABLE "company_details" ADD CONSTRAINT "company_details_ticker_companies_ticker_fk" FOREIGN KEY ("ticker") REFERENCES "public"."companies"("ticker") ON DELETE no action ON UPDATE no action;
EXCEPTION
 WHEN duplicate_object THEN null;
END $$;
--> statement-breakpoint
DO $$ BEGIN
 ALTER TABLE "market_snapshots" ADD CONSTRAINT "market_snapshots_ticker_companies_ticker_fk" FOREIGN KEY ("ticker") REFERENCES "public"."companies"("ticker") ON DELETE no action ON UPDATE no action;
EXCEPTION
 WHEN duplicate_object THEN null;
END $$;
--> statement-breakpoint
DO $$ BEGIN
 ALTER TABLE "auth_sessions" ADD CONSTRAINT "auth_sessions_user_id_users_id_fk" FOREIGN KEY ("user_id") REFERENCES "public"."users"("id") ON DELETE no action ON UPDATE no action;
EXCEPTION
 WHEN duplicate_object THEN null;
END $$;
--> statement-breakpoint
DO $$ BEGIN
 ALTER TABLE "company_profiles" ADD CONSTRAINT "company_profiles_user_id_users_id_fk" FOREIGN KEY ("user_id") REFERENCES "public"."users"("id") ON DELETE no action ON UPDATE no action;
EXCEPTION
 WHEN duplicate_object THEN null;
END $$;
--> statement-breakpoint
DO $$ BEGIN
 ALTER TABLE "profile_events" ADD CONSTRAINT "profile_events_user_id_users_id_fk" FOREIGN KEY ("user_id") REFERENCES "public"."users"("id") ON DELETE no action ON UPDATE no action;
EXCEPTION
 WHEN duplicate_object THEN null;
END $$;
--> statement-breakpoint
DO $$ BEGIN
 ALTER TABLE "research_sessions" ADD CONSTRAINT "research_sessions_user_id_users_id_fk" FOREIGN KEY ("user_id") REFERENCES "public"."users"("id") ON DELETE no action ON UPDATE no action;
EXCEPTION
 WHEN duplicate_object THEN null;
END $$;
--> statement-breakpoint
DO $$ BEGIN
 ALTER TABLE "research_sessions" ADD CONSTRAINT "research_sessions_ticker_companies_ticker_fk" FOREIGN KEY ("ticker") REFERENCES "public"."companies"("ticker") ON DELETE no action ON UPDATE no action;
EXCEPTION
 WHEN duplicate_object THEN null;
END $$;
--> statement-breakpoint
DO $$ BEGIN
 ALTER TABLE "research_snapshots" ADD CONSTRAINT "research_snapshots_user_id_users_id_fk" FOREIGN KEY ("user_id") REFERENCES "public"."users"("id") ON DELETE no action ON UPDATE no action;
EXCEPTION
 WHEN duplicate_object THEN null;
END $$;
--> statement-breakpoint
DO $$ BEGIN
 ALTER TABLE "portfolio_positions" ADD CONSTRAINT "portfolio_positions_user_id_users_id_fk" FOREIGN KEY ("user_id") REFERENCES "public"."users"("id") ON DELETE no action ON UPDATE no action;
EXCEPTION
 WHEN duplicate_object THEN null;
END $$;
--> statement-breakpoint
DO $$ BEGIN
 ALTER TABLE "portfolio_snapshot_totals" ADD CONSTRAINT "portfolio_snapshot_totals_user_id_users_id_fk" FOREIGN KEY ("user_id") REFERENCES "public"."users"("id") ON DELETE no action ON UPDATE no action;
EXCEPTION
 WHEN duplicate_object THEN null;
END $$;
--> statement-breakpoint
DO $$ BEGIN
 ALTER TABLE "portfolio_snapshots" ADD CONSTRAINT "portfolio_snapshots_user_id_users_id_fk" FOREIGN KEY ("user_id") REFERENCES "public"."users"("id") ON DELETE no action ON UPDATE no action;
EXCEPTION
 WHEN duplicate_object THEN null;
END $$;
--> statement-breakpoint
DO $$ BEGIN
 ALTER TABLE "watch_rules" ADD CONSTRAINT "watch_rules_user_id_users_id_fk" FOREIGN KEY ("user_id") REFERENCES "public"."users"("id") ON DELETE no action ON UPDATE no action;
EXCEPTION
 WHEN duplicate_object THEN null;
END $$;
--> statement-breakpoint
DO $$ BEGIN
 ALTER TABLE "watchlist_prefs" ADD CONSTRAINT "watchlist_prefs_user_id_users_id_fk" FOREIGN KEY ("user_id") REFERENCES "public"."users"("id") ON DELETE no action ON UPDATE no action;
EXCEPTION
 WHEN duplicate_object THEN null;
END $$;
--> statement-breakpoint
DO $$ BEGIN
 ALTER TABLE "cn_filing_ingest_log" ADD CONSTRAINT "cn_filing_ingest_log_ticker_companies_ticker_fk" FOREIGN KEY ("ticker") REFERENCES "public"."companies"("ticker") ON DELETE no action ON UPDATE no action;
EXCEPTION
 WHEN duplicate_object THEN null;
END $$;
--> statement-breakpoint
DO $$ BEGIN
 ALTER TABLE "cn_financials" ADD CONSTRAINT "cn_financials_ticker_companies_ticker_fk" FOREIGN KEY ("ticker") REFERENCES "public"."companies"("ticker") ON DELETE no action ON UPDATE no action;
EXCEPTION
 WHEN duplicate_object THEN null;
END $$;
--> statement-breakpoint
DO $$ BEGIN
 ALTER TABLE "comp_peers" ADD CONSTRAINT "comp_peers_ticker_companies_ticker_fk" FOREIGN KEY ("ticker") REFERENCES "public"."companies"("ticker") ON DELETE no action ON UPDATE no action;
EXCEPTION
 WHEN duplicate_object THEN null;
END $$;
--> statement-breakpoint
DO $$ BEGIN
 ALTER TABLE "earnings_calendar" ADD CONSTRAINT "earnings_calendar_ticker_companies_ticker_fk" FOREIGN KEY ("ticker") REFERENCES "public"."companies"("ticker") ON DELETE no action ON UPDATE no action;
EXCEPTION
 WHEN duplicate_object THEN null;
END $$;
--> statement-breakpoint
DO $$ BEGIN
 ALTER TABLE "historical_valuation" ADD CONSTRAINT "historical_valuation_ticker_companies_ticker_fk" FOREIGN KEY ("ticker") REFERENCES "public"."companies"("ticker") ON DELETE no action ON UPDATE no action;
EXCEPTION
 WHEN duplicate_object THEN null;
END $$;
--> statement-breakpoint
DO $$ BEGIN
 ALTER TABLE "historical_valuation_points" ADD CONSTRAINT "historical_valuation_points_ticker_companies_ticker_fk" FOREIGN KEY ("ticker") REFERENCES "public"."companies"("ticker") ON DELETE no action ON UPDATE no action;
EXCEPTION
 WHEN duplicate_object THEN null;
END $$;
--> statement-breakpoint
DO $$ BEGIN
 ALTER TABLE "hk_buybacks" ADD CONSTRAINT "hk_buybacks_ticker_companies_ticker_fk" FOREIGN KEY ("ticker") REFERENCES "public"."companies"("ticker") ON DELETE no action ON UPDATE no action;
EXCEPTION
 WHEN duplicate_object THEN null;
END $$;
--> statement-breakpoint
DO $$ BEGIN
 ALTER TABLE "hk_filing_ingest_log" ADD CONSTRAINT "hk_filing_ingest_log_ticker_companies_ticker_fk" FOREIGN KEY ("ticker") REFERENCES "public"."companies"("ticker") ON DELETE no action ON UPDATE no action;
EXCEPTION
 WHEN duplicate_object THEN null;
END $$;
--> statement-breakpoint
DO $$ BEGIN
 ALTER TABLE "hk_financials" ADD CONSTRAINT "hk_financials_ticker_companies_ticker_fk" FOREIGN KEY ("ticker") REFERENCES "public"."companies"("ticker") ON DELETE no action ON UPDATE no action;
EXCEPTION
 WHEN duplicate_object THEN null;
END $$;
--> statement-breakpoint
DO $$ BEGIN
 ALTER TABLE "insider_activity" ADD CONSTRAINT "insider_activity_ticker_companies_ticker_fk" FOREIGN KEY ("ticker") REFERENCES "public"."companies"("ticker") ON DELETE no action ON UPDATE no action;
EXCEPTION
 WHEN duplicate_object THEN null;
END $$;
--> statement-breakpoint
DO $$ BEGIN
 ALTER TABLE "web_evidence" ADD CONSTRAINT "web_evidence_ticker_companies_ticker_fk" FOREIGN KEY ("ticker") REFERENCES "public"."companies"("ticker") ON DELETE no action ON UPDATE no action;
EXCEPTION
 WHEN duplicate_object THEN null;
END $$;
--> statement-breakpoint
DO $$ BEGIN
 ALTER TABLE "notifications" ADD CONSTRAINT "notifications_user_id_users_id_fk" FOREIGN KEY ("user_id") REFERENCES "public"."users"("id") ON DELETE no action ON UPDATE no action;
EXCEPTION
 WHEN duplicate_object THEN null;
END $$;
--> statement-breakpoint
DO $$ BEGIN
 ALTER TABLE "notifications" ADD CONSTRAINT "notifications_ticker_companies_ticker_fk" FOREIGN KEY ("ticker") REFERENCES "public"."companies"("ticker") ON DELETE no action ON UPDATE no action;
EXCEPTION
 WHEN duplicate_object THEN null;
END $$;
--> statement-breakpoint
DO $$ BEGIN
 ALTER TABLE "documents" ADD CONSTRAINT "documents_user_id_users_id_fk" FOREIGN KEY ("user_id") REFERENCES "public"."users"("id") ON DELETE no action ON UPDATE no action;
EXCEPTION
 WHEN duplicate_object THEN null;
END $$;
--> statement-breakpoint
DO $$ BEGIN
 ALTER TABLE "documents" ADD CONSTRAINT "documents_ticker_companies_ticker_fk" FOREIGN KEY ("ticker") REFERENCES "public"."companies"("ticker") ON DELETE no action ON UPDATE no action;
EXCEPTION
 WHEN duplicate_object THEN null;
END $$;
--> statement-breakpoint
DO $$ BEGIN
 ALTER TABLE "feedback" ADD CONSTRAINT "feedback_user_id_users_id_fk" FOREIGN KEY ("user_id") REFERENCES "public"."users"("id") ON DELETE no action ON UPDATE no action;
EXCEPTION
 WHEN duplicate_object THEN null;
END $$;
--> statement-breakpoint
DO $$ BEGIN
 ALTER TABLE "llm_audit" ADD CONSTRAINT "llm_audit_user_id_users_id_fk" FOREIGN KEY ("user_id") REFERENCES "public"."users"("id") ON DELETE no action ON UPDATE no action;
EXCEPTION
 WHEN duplicate_object THEN null;
END $$;
--> statement-breakpoint
DO $$ BEGIN
 ALTER TABLE "user_preferences" ADD CONSTRAINT "user_preferences_user_id_users_id_fk" FOREIGN KEY ("user_id") REFERENCES "public"."users"("id") ON DELETE no action ON UPDATE no action;
EXCEPTION
 WHEN duplicate_object THEN null;
END $$;
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "idx_market_ticker" ON "market_snapshots" USING btree ("ticker");--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "idx_market_valid_time" ON "market_snapshots" USING btree ("valid_time");--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "idx_auth_sessions_user" ON "auth_sessions" USING btree ("user_id");--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "idx_auth_sessions_expires" ON "auth_sessions" USING btree ("expires_at");--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "idx_profile_events_ticker" ON "profile_events" USING btree ("ticker","id");--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "idx_profile_events_user" ON "profile_events" USING btree ("user_id","ticker","id");--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "idx_sessions_ticker" ON "research_sessions" USING btree ("ticker");--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "idx_sessions_user" ON "research_sessions" USING btree ("user_id","updated_at");--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "idx_research_snapshots_ticker" ON "research_snapshots" USING btree ("ticker","valid_time");--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "idx_research_snapshots_user" ON "research_snapshots" USING btree ("user_id","ticker","valid_time");--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "idx_portfolio_snapshot_totals_snapshot" ON "portfolio_snapshot_totals" USING btree ("user_id","snapshot_valid_time");--> statement-breakpoint
CREATE UNIQUE INDEX IF NOT EXISTS "uq_portfolio_snapshot_totals_snapshot_currency" ON "portfolio_snapshot_totals" USING btree ("user_id","snapshot_valid_time","currency");--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "idx_watch_rules_ticker" ON "watch_rules" USING btree ("ticker","active");--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "idx_watch_rules_user" ON "watch_rules" USING btree ("user_id","active","ticker");--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "idx_cn_financials_ticker" ON "cn_financials" USING btree ("ticker","valid_time");--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "idx_historical_valuation_points_ticker" ON "historical_valuation_points" USING btree ("ticker","period_end_date");--> statement-breakpoint
CREATE UNIQUE INDEX IF NOT EXISTS "uq_historical_valuation_points_ticker_period" ON "historical_valuation_points" USING btree ("ticker","period_end_date");--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "idx_hk_buybacks_ticker_date" ON "hk_buybacks" USING btree ("ticker","trade_date");--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "idx_hk_financials_ticker" ON "hk_financials" USING btree ("ticker","valid_time");--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "idx_web_evidence_ticker_intent" ON "web_evidence" USING btree ("ticker","intent");--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "idx_web_evidence_url" ON "web_evidence" USING btree ("url");--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "idx_web_evidence_knowledge_time" ON "web_evidence" USING btree ("knowledge_time");--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "idx_notif_read" ON "notifications" USING btree ("read_at");--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "idx_notif_dedupe" ON "notifications" USING btree ("dedupe_key","created_at");--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "idx_notif_user" ON "notifications" USING btree ("user_id","read_at");--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "idx_canary_batch" ON "canary_runs" USING btree ("batch_id");--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "idx_canary_source" ON "canary_runs" USING btree ("source","created_at");--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "idx_documents_user" ON "documents" USING btree ("user_id","created_at");--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "idx_fact_guard_audit_created" ON "fact_guard_audit" USING btree ("created_at");--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "idx_feedback_user_time" ON "feedback" USING btree ("user_id","created_at");--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "idx_llm_audit_provider" ON "llm_audit" USING btree ("provider","created_at");--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "idx_llm_audit_user_time" ON "llm_audit" USING btree ("user_id","created_at");