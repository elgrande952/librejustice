ALTER TABLE decisions
    ADD COLUMN IF NOT EXISTS full_text     text,
    ADD COLUMN IF NOT EXISTS source_fields jsonb,
    ADD COLUMN IF NOT EXISTS embed_version smallint;

ALTER TABLE decisions
    ALTER COLUMN full_text SET COMPRESSION pglz;
