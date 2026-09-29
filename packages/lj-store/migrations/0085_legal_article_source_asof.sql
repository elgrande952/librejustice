ALTER TABLE legal_article
    ADD COLUMN IF NOT EXISTS source_asof         DATE,
    ADD COLUMN IF NOT EXISTS source_upstream_url TEXT;

CREATE TABLE IF NOT EXISTS ingest_freshness (
    source TEXT PRIMARY KEY,
    asof   DATE NOT NULL
);

INSERT INTO ingest_freshness (source, asof) VALUES
    ('legifrance', CURRENT_DATE),
    ('kali',       CURRENT_DATE),
    ('jorf',       CURRENT_DATE)
ON CONFLICT (source) DO NOTHING;

UPDATE legal_article SET source_asof = DATE '2025-10-04'
    WHERE source = 'jafbase' AND source_asof IS NULL;
