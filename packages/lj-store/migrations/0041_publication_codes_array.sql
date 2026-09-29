SET LOCAL maintenance_work_mem = '2GB';
SET LOCAL max_parallel_maintenance_workers = 4;

ALTER TABLE decisions
    ADD COLUMN publication_codes TEXT[] NOT NULL DEFAULT '{}';

UPDATE decisions
SET publication_codes = ARRAY[publication_code]
WHERE publication_code IS NOT NULL AND publication_code <> '';

DROP TRIGGER IF EXISTS decisions_sync_chunks_meta ON decisions;

ALTER TABLE decisions
    DROP COLUMN is_recueil,
    DROP COLUMN is_tables_lebon,
    DROP COLUMN publication_code;

UPDATE decisions SET docket_numbers = '{}' WHERE docket_numbers IS NULL;
ALTER TABLE decisions
    ALTER COLUMN docket_numbers SET DEFAULT '{}',
    ALTER COLUMN docket_numbers SET NOT NULL;

ALTER TABLE decision_chunks
    ADD COLUMN publication_codes TEXT[] NOT NULL DEFAULT '{}';

UPDATE decision_chunks c
SET publication_codes = d.publication_codes
FROM decisions d
WHERE c.decision_id = d.id
  AND c.publication_codes IS DISTINCT FROM d.publication_codes;

ALTER TABLE decision_chunks
    DROP COLUMN is_recueil,
    DROP COLUMN is_tables_lebon;

CREATE OR REPLACE FUNCTION sync_chunks_decision_meta() RETURNS TRIGGER AS $$
BEGIN
    UPDATE decision_chunks SET
        jurisdiction_level = NEW.jurisdiction_level,
        instance_level     = NEW.instance_level,
        main_outcome       = NEW.main_outcome,
        special_procedure  = NEW.special_procedure,
        jurisdiction_name  = NEW.jurisdiction_name,
        publication_codes  = NEW.publication_codes,
        date_lecture       = NEW.date_lecture
    WHERE decision_id = NEW.id;
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER decisions_sync_chunks_meta
    AFTER UPDATE ON decisions
    FOR EACH ROW
    WHEN (
        OLD.jurisdiction_level IS DISTINCT FROM NEW.jurisdiction_level
     OR OLD.instance_level     IS DISTINCT FROM NEW.instance_level
     OR OLD.main_outcome       IS DISTINCT FROM NEW.main_outcome
     OR OLD.special_procedure  IS DISTINCT FROM NEW.special_procedure
     OR OLD.jurisdiction_name  IS DISTINCT FROM NEW.jurisdiction_name
     OR OLD.publication_codes  IS DISTINCT FROM NEW.publication_codes
     OR OLD.date_lecture       IS DISTINCT FROM NEW.date_lecture
    )
    EXECUTE FUNCTION sync_chunks_decision_meta();

DROP INDEX IF EXISTS chunks_bm25;

CREATE INDEX chunks_bm25 ON decision_chunks
USING bm25 (
    id,
    body,
    (juridiction_type::pdb.literal),
    (jurisdiction_level::pdb.literal),
    (instance_level::pdb.literal),
    (main_outcome::pdb.literal),
    (special_procedure::pdb.literal),
    (jurisdiction_name::pdb.literal_normalized),
    (legal_instruments::pdb.literal),
    (legal_article_labels::pdb.literal),
    (publication_codes::pdb.literal),
    date_lecture
)
WITH (
  key_field = 'id',
  text_fields = '{"body": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true}, "record": "position"}}'
);
