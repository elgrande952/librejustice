CREATE TABLE citation_resolution_override (
    id           BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    text_key     TEXT NOT NULL,
    article_key  TEXT,
    ref_text_uid TEXT,
    ref_num_key  TEXT,
    version      INT  NOT NULL,
    reason       TEXT NOT NULL,
    source       TEXT NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_cro_key ON citation_resolution_override (lower(text_key), article_key);

ALTER TABLE decisions ADD COLUMN certified_version INT;
