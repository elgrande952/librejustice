CREATE EXTENSION IF NOT EXISTS pg_trgm;

CREATE INDEX idx_legal_text_title_key_trgm
    ON legal_text USING gin (title_key gin_trgm_ops);
