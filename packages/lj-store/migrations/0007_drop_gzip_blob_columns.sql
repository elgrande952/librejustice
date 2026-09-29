ALTER TABLE decision_full_text DROP COLUMN IF EXISTS gzip_blob;
ALTER TABLE decision_full_text DROP COLUMN IF EXISTS raw_bytes;
ALTER TABLE decision_full_text DROP COLUMN IF EXISTS gzip_bytes;
