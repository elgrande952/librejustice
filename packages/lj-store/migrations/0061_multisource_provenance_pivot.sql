ALTER TABLE decision_sources
    ADD COLUMN IF NOT EXISTS source_fields jsonb,
    ADD COLUMN IF NOT EXISTS deleted_at    timestamptz;

ALTER TABLE decision_sources
    ALTER COLUMN source_fields SET COMPRESSION pglz;

CREATE INDEX IF NOT EXISTS idx_decision_sources_active
    ON decision_sources (decision_id)
    WHERE deleted_at IS NULL;

ALTER TABLE decisions
    ADD COLUMN IF NOT EXISTS identity_key text,
    ADD COLUMN IF NOT EXISTS deleted_at   timestamptz;

CREATE INDEX IF NOT EXISTS idx_decisions_identity_key
    ON decisions (identity_key)
    WHERE identity_key IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_decisions_active
    ON decisions (id)
    WHERE deleted_at IS NULL;
