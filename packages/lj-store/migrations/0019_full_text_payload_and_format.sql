ALTER TABLE decision_full_text RENAME COLUMN xml_gzip TO source_payload_gzip;

ALTER TABLE decision_full_text
  ADD COLUMN payload_format TEXT NOT NULL DEFAULT 'xml'
    CHECK (payload_format IN ('xml', 'json'));

ALTER TABLE decision_full_text ALTER COLUMN payload_format DROP DEFAULT;
