ALTER TABLE legal_text ADD COLUMN IF NOT EXISTS eli            TEXT;
ALTER TABLE legal_text ADD COLUMN IF NOT EXISTS nor            TEXT;
ALTER TABLE legal_text ADD COLUMN IF NOT EXISTS instrument_key TEXT;

CREATE INDEX IF NOT EXISTS idx_legal_text_eli ON legal_text (eli) WHERE eli IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_legal_text_nor ON legal_text (nor) WHERE nor IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_legal_text_instrument_key
    ON legal_text (instrument_key) WHERE instrument_key IS NOT NULL;
