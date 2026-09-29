CREATE TABLE IF NOT EXISTS article_commentaire (
    id               BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    text_uid         TEXT NOT NULL,
    num_key          TEXT,
    source           TEXT NOT NULL,
    source_uid       TEXT NOT NULL UNIQUE,
    content_checksum TEXT NOT NULL,
    payload_format   TEXT NOT NULL DEFAULT 'json',
    source_fields    JSONB NOT NULL,
    ingested_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    deleted_at       TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS article_commentaire_anchor_idx
    ON article_commentaire (text_uid, num_key)
    WHERE deleted_at IS NULL;
