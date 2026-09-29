CREATE EXTENSION IF NOT EXISTS pg_search;
CREATE EXTENSION IF NOT EXISTS vchord CASCADE;

CREATE TABLE decisions (
    id               BIGSERIAL PRIMARY KEY,
    source_uid       TEXT        NOT NULL UNIQUE,
    juridiction_type TEXT        NOT NULL,
    content_hash     TEXT        NOT NULL,
    deleted_at       TIMESTAMPTZ,
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_decisions_jur ON decisions (juridiction_type);

CREATE TABLE decision_chunks (
    id               BIGSERIAL PRIMARY KEY,
    decision_id      BIGINT      NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    chunk_index      INTEGER     NOT NULL,
    juridiction_type TEXT        NOT NULL,
    char_start       INTEGER     NOT NULL,
    char_end         INTEGER     NOT NULL,
    body             TEXT        NOT NULL,
    embedding        vector(1024),
    UNIQUE (decision_id, chunk_index)
);

CREATE INDEX idx_decision_chunks_decision_id ON decision_chunks (decision_id);

CREATE INDEX chunks_bm25 ON decision_chunks
USING bm25 (id, body, juridiction_type)
WITH (
  key_field = 'id',
  text_fields = '{"body": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true}, "record": "position"}}'
);

CREATE INDEX chunks_vec ON decision_chunks
USING vchordrq (embedding vector_cosine_ops);

CREATE TABLE decision_full_text (
    decision_id  BIGINT  PRIMARY KEY REFERENCES decisions(id) ON DELETE CASCADE,
    gzip_blob    BYTEA   NOT NULL,
    raw_bytes    INTEGER NOT NULL,
    gzip_bytes   INTEGER NOT NULL
);
