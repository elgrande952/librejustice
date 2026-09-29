ALTER TABLE decision_full_text ALTER COLUMN gzip_blob  DROP NOT NULL;
ALTER TABLE decision_full_text ALTER COLUMN raw_bytes  DROP NOT NULL;
ALTER TABLE decision_full_text ALTER COLUMN gzip_bytes DROP NOT NULL;
