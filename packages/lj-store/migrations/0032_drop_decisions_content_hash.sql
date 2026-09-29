ALTER TABLE decisions ALTER COLUMN content_checksum SET NOT NULL;
ALTER TABLE decisions DROP COLUMN content_hash;
