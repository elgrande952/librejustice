ALTER TABLE annonce ADD COLUMN zones text[] NOT NULL DEFAULT '{}';

CREATE INDEX annonce_zones_gin ON annonce USING gin (zones);
