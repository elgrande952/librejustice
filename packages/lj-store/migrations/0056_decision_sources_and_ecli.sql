ALTER TABLE decisions
ADD COLUMN IF NOT EXISTS ecli TEXT;

CREATE UNIQUE INDEX IF NOT EXISTS idx_decisions_ecli
ON decisions (ecli)
WHERE ecli IS NOT NULL;

CREATE TABLE decision_sources (
    id               BIGSERIAL PRIMARY KEY,
    decision_id      BIGINT   NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    source           TEXT     NOT NULL,
    source_uid       TEXT     NOT NULL UNIQUE,
    content_checksum TEXT     NOT NULL,
    source_rank      SMALLINT NOT NULL,
    payload_format   TEXT     NOT NULL CHECK (payload_format IN ('xml','json','dila-xml','html')),
    ingested_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_decision_sources_decision_id ON decision_sources (decision_id);
