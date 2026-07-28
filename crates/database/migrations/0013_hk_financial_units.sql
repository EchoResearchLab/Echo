ALTER TABLE hk_financials
    ADD COLUMN IF NOT EXISTS source_unit_scale numeric,
    ADD COLUMN IF NOT EXISTS amounts_normalized boolean NOT NULL DEFAULT false,
    ADD COLUMN IF NOT EXISTS parser_version text;

COMMENT ON COLUMN hk_financials.source_unit_scale IS
    '公告原始金额单位倍率（1/1000/1000000/1000000000）；仅作来源追溯，金额列已换算为绝对值';
COMMENT ON COLUMN hk_financials.amounts_normalized IS
    '金额列是否已由受控 ingest 校验并换算为绝对值；历史未知行保持 false';
COMMENT ON COLUMN hk_financials.parser_version IS
    '完成单位识别与金额归一化的解析器版本';

ALTER TABLE hk_financials
    ADD CONSTRAINT hk_financials_source_unit_scale_valid
    CHECK (
        source_unit_scale IS NULL
        OR source_unit_scale IN (1, 1000, 1000000, 1000000000)
    );

CREATE INDEX IF NOT EXISTS idx_hk_financials_trusted_latest
    ON hk_financials (ticker, knowledge_time DESC, id DESC)
    WHERE amounts_normalized = true;
