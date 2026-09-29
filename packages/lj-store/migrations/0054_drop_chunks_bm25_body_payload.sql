DROP INDEX IF EXISTS chunks_bm25;

ALTER TABLE decision_chunks DROP COLUMN IF EXISTS body;

ALTER TABLE decision_full_text DROP COLUMN IF EXISTS source_payload_gzip;
