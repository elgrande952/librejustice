ALTER TABLE decision_full_text
ADD COLUMN IF NOT EXISTS source_xml_gzip BYTEA;
