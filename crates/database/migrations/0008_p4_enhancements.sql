-- P4: quiet hours for notification preferences
ALTER TABLE user_preferences ADD COLUMN IF NOT EXISTS quiet_hours_start text;
ALTER TABLE user_preferences ADD COLUMN IF NOT EXISTS quiet_hours_end text;

-- P4: research memory tables
CREATE TABLE IF NOT EXISTS research_facts (
  id bigserial PRIMARY KEY,
  user_id text NOT NULL DEFAULT 'local' REFERENCES users(id),
  ticker text NOT NULL REFERENCES companies(ticker),
  fact text NOT NULL,
  source text,
  confidence text DEFAULT 'confirmed',
  session_id text,
  valid_from timestamp with time zone,
  superseded_by bigint,
  created_at timestamp with time zone NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_research_facts_user_ticker ON research_facts(user_id, ticker);

CREATE TABLE IF NOT EXISTS research_questions (
  id bigserial PRIMARY KEY,
  user_id text NOT NULL DEFAULT 'local' REFERENCES users(id),
  ticker text NOT NULL REFERENCES companies(ticker),
  question text NOT NULL,
  status text NOT NULL DEFAULT 'open',
  answer text,
  session_id text,
  created_at timestamp with time zone NOT NULL DEFAULT now(),
  resolved_at timestamp with time zone
);
CREATE INDEX IF NOT EXISTS idx_research_questions_user_ticker ON research_questions(user_id, ticker);

CREATE TABLE IF NOT EXISTS review_dates (
  id bigserial PRIMARY KEY,
  user_id text NOT NULL DEFAULT 'local' REFERENCES users(id),
  ticker text NOT NULL REFERENCES companies(ticker),
  review_date date NOT NULL,
  reason text,
  completed boolean NOT NULL DEFAULT false,
  created_at timestamp with time zone NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_review_dates_user ON review_dates(user_id, review_date);

-- P4: feedback queue enhancement
ALTER TABLE feedback ADD COLUMN IF NOT EXISTS category text DEFAULT 'general';
ALTER TABLE feedback ADD COLUMN IF NOT EXISTS ticker text;
ALTER TABLE feedback ADD COLUMN IF NOT EXISTS session_id text;
ALTER TABLE feedback ADD COLUMN IF NOT EXISTS resolved_at timestamp with time zone;

-- RLS policies for new tables
ALTER TABLE research_facts ENABLE ROW LEVEL SECURITY;
CREATE POLICY research_facts_tenant ON research_facts USING (user_id = current_setting('app.user_id'));

ALTER TABLE research_questions ENABLE ROW LEVEL SECURITY;
CREATE POLICY research_questions_tenant ON research_questions USING (user_id = current_setting('app.user_id'));

ALTER TABLE review_dates ENABLE ROW LEVEL SECURITY;
CREATE POLICY review_dates_tenant ON review_dates USING (user_id = current_setting('app.user_id'));
