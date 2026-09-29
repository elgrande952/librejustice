DO $$
DECLARE
    missing_prov   bigint;
    missing_fields bigint;
BEGIN
    SELECT count(*) INTO missing_prov
    FROM decisions d
    WHERE NOT EXISTS (
        SELECT 1 FROM decision_sources s WHERE s.source_uid = d.source_uid
    );

    SELECT count(*) INTO missing_fields
    FROM decisions d
    WHERE d.source_fields IS NOT NULL
      AND NOT EXISTS (
        SELECT 1 FROM decision_sources s
        WHERE s.source_uid = d.source_uid AND s.source_fields IS NOT NULL
      );

    IF missing_prov > 0 OR missing_fields > 0 THEN
        RAISE EXCEPTION
            'Portage incomplet — % décision(s) sans provenance, % source_fields non porté(s). Lancer `lj-ingest dedup-backfill` (ou `backfill-decision-sources`) AVANT le DROP.',
            missing_prov, missing_fields;
    END IF;
END $$;

ALTER TABLE decisions
    DROP COLUMN IF EXISTS source_uid,
    DROP COLUMN IF EXISTS content_checksum,
    DROP COLUMN IF EXISTS source_fields;
