ALTER TABLE decisions ADD COLUMN chamber_key TEXT;

CREATE INDEX idx_decisions_chamber_key ON decisions (jurisdiction_code, chamber_key)
  WHERE deleted_at IS NULL AND chamber_key IS NOT NULL;
