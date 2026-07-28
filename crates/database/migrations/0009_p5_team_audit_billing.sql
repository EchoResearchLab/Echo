-- P5: Team/organization support
CREATE TABLE IF NOT EXISTS teams (
  id text PRIMARY KEY,
  name text NOT NULL,
  slug text NOT NULL UNIQUE,
  owner_id text NOT NULL REFERENCES users(id),
  created_at timestamp with time zone NOT NULL DEFAULT now(),
  updated_at timestamp with time zone NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS team_memberships (
  team_id text NOT NULL REFERENCES teams(id),
  user_id text NOT NULL REFERENCES users(id),
  role text NOT NULL DEFAULT 'viewer',
  created_at timestamp with time zone NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX IF NOT EXISTS uq_team_membership ON team_memberships(team_id, user_id);
CREATE INDEX IF NOT EXISTS idx_team_memberships_user ON team_memberships(user_id);

-- P5: Audit log
CREATE TABLE IF NOT EXISTS audit_log (
  id bigserial PRIMARY KEY,
  user_id text NOT NULL REFERENCES users(id),
  action text NOT NULL,
  resource text NOT NULL,
  resource_id text,
  detail jsonb,
  ip_address text,
  created_at timestamp with time zone NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_audit_log_user ON audit_log(user_id, created_at);
CREATE INDEX IF NOT EXISTS idx_audit_log_action ON audit_log(action, created_at);
CREATE INDEX IF NOT EXISTS idx_audit_log_resource ON audit_log(resource, resource_id);

-- P5: Billing
CREATE TABLE IF NOT EXISTS plans (
  id text PRIMARY KEY,
  name text NOT NULL,
  tier text NOT NULL,
  monthly_price_usd numeric NOT NULL,
  yearly_price_usd numeric,
  max_daily_calls integer NOT NULL,
  max_daily_cost_usd numeric,
  max_team_members integer NOT NULL DEFAULT 1,
  features text[],
  active boolean NOT NULL DEFAULT true,
  created_at timestamp with time zone NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS subscriptions (
  id text PRIMARY KEY,
  user_id text NOT NULL REFERENCES users(id),
  plan_id text NOT NULL REFERENCES plans(id),
  status text NOT NULL DEFAULT 'active',
  current_period_start timestamp with time zone NOT NULL,
  current_period_end timestamp with time zone NOT NULL,
  canceled_at timestamp with time zone,
  external_id text,
  created_at timestamp with time zone NOT NULL DEFAULT now(),
  updated_at timestamp with time zone NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_subscriptions_user ON subscriptions(user_id, status);
CREATE INDEX IF NOT EXISTS idx_subscriptions_external ON subscriptions(external_id);

-- P5: Data source field-level registry
CREATE TABLE IF NOT EXISTS data_source_fields (
  source text NOT NULL,
  field text NOT NULL,
  description text,
  license_tier text NOT NULL DEFAULT 'unlicensed_free_tier',
  commercial_use_allowed boolean NOT NULL DEFAULT false,
  coverage text,
  notes text,
  updated_at timestamp with time zone NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX IF NOT EXISTS uq_data_source_field ON data_source_fields(source, field);
CREATE INDEX IF NOT EXISTS idx_data_source_fields_source ON data_source_fields(source);

-- Seed default plans
INSERT INTO plans (id, name, tier, monthly_price_usd, yearly_price_usd, max_daily_calls, max_daily_cost_usd, max_team_members, features, active)
VALUES
  ('free', '免费版', 'free', 0, 0, 10, 0, 1, ARRAY['基础研究', '看盘', '通知'], true),
  ('pro', '专业版', 'pro', 29, 290, 100, 5, 5, ARRAY['基础研究', '看盘', '通知', '深度报告', '导出', 'API'], true),
  ('team', '团队版', 'team', 99, 990, 500, 25, 25, ARRAY['基础研究', '看盘', '通知', '深度报告', '导出', 'API', '团队空间', '审计日志'], true)
ON CONFLICT (id) DO NOTHING;
