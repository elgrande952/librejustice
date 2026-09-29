ALTER TABLE decision_full_text DROP COLUMN IF EXISTS gzip_blob;
ALTER TABLE decision_full_text DROP COLUMN IF EXISTS raw_bytes;
ALTER TABLE decision_full_text DROP COLUMN IF EXISTS gzip_bytes;
ALTER TABLE decision_full_text RENAME COLUMN source_xml_gzip TO xml_gzip;
