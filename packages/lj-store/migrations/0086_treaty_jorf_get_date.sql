UPDATE legal_article SET source_asof = CURRENT_DATE
    WHERE source IN ('treaty', 'jorf') AND source_asof IS NULL;

DELETE FROM ingest_freshness WHERE source = 'jorf';
