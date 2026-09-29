DROP INDEX IF EXISTS idx_decisions_ecli;

CREATE INDEX IF NOT EXISTS idx_decisions_ecli
ON decisions (ecli)
WHERE ecli IS NOT NULL;
