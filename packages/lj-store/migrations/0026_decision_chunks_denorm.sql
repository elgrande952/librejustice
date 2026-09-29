SET LOCAL maintenance_work_mem = '2GB';
SET LOCAL max_parallel_maintenance_workers = 4;

DELETE FROM decisions WHERE deleted_at IS NOT NULL;

DROP INDEX IF EXISTS idx_decisions_date_lecture;
DROP INDEX IF EXISTS idx_decisions_jurisdiction_level;
DROP INDEX IF EXISTS idx_decisions_instance_level;
DROP INDEX IF EXISTS idx_decisions_main_outcome;
DROP INDEX IF EXISTS idx_decisions_special_procedure;
DROP INDEX IF EXISTS idx_decisions_is_recueil;
DROP INDEX IF EXISTS idx_decisions_docket_numbers;
DROP INDEX IF EXISTS idx_decisions_legal_references;

ALTER TABLE decisions DROP COLUMN deleted_at;

CREATE INDEX idx_decisions_date_lecture        ON decisions (date_lecture);
CREATE INDEX idx_decisions_jurisdiction_level  ON decisions (jurisdiction_level);
CREATE INDEX idx_decisions_instance_level      ON decisions (instance_level);
CREATE INDEX idx_decisions_main_outcome        ON decisions (main_outcome);
CREATE INDEX idx_decisions_special_procedure   ON decisions (special_procedure);
CREATE INDEX idx_decisions_is_recueil          ON decisions (is_recueil);
CREATE INDEX idx_decisions_docket_numbers      ON decisions USING GIN (docket_numbers);

CREATE TABLE decision_legal_references (
    decision_id BIGINT NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    instrument  TEXT   NOT NULL,
    article     TEXT
);

CREATE INDEX idx_dlr_decision   ON decision_legal_references (decision_id);
CREATE INDEX idx_dlr_article    ON decision_legal_references (article text_pattern_ops)
  WHERE article IS NOT NULL;

CREATE INDEX idx_dlr_instrument ON decision_legal_references USING hash (instrument);

WITH src AS (
  SELECT
    d.id AS decision_id,
    CASE
      WHEN length(lr->>'instrument') > 200
        THEN split_part(lr->>'instrument', '. ', 1)
      ELSE lr->>'instrument'
    END AS instrument,
    lr->'articles' AS articles
  FROM decisions d,
       jsonb_array_elements(COALESCE(d.legal_references, '[]'::jsonb)) AS lr
  WHERE lr->>'instrument' IS NOT NULL
)
INSERT INTO decision_legal_references (decision_id, instrument, article)
SELECT
    src.decision_id,
    src.instrument,
    art_text
FROM src
LEFT JOIN LATERAL jsonb_array_elements_text(
    CASE
      WHEN jsonb_typeof(src.articles) = 'array'
        AND jsonb_array_length(src.articles) > 0
      THEN src.articles
      ELSE '[null]'::jsonb
    END
) AS art_text ON TRUE
WHERE length(src.instrument) BETWEEN 1 AND 500;

ALTER TABLE decisions DROP COLUMN legal_references;

DROP INDEX IF EXISTS chunks_vec;
DROP INDEX IF EXISTS chunks_bm25;

ALTER TABLE decision_chunks
    ADD COLUMN jurisdiction_level TEXT,
    ADD COLUMN instance_level     TEXT,
    ADD COLUMN main_outcome       TEXT,
    ADD COLUMN special_procedure  TEXT,
    ADD COLUMN jurisdiction_name  TEXT,
    ADD COLUMN is_recueil         BOOLEAN,
    ADD COLUMN is_tables_lebon    BOOLEAN,
    ADD COLUMN date_lecture       DATE,
    ADD COLUMN legal_instruments  TEXT[];

UPDATE decision_chunks c SET
    jurisdiction_level = d.jurisdiction_level,
    instance_level     = d.instance_level,
    main_outcome       = d.main_outcome,
    special_procedure  = d.special_procedure,
    jurisdiction_name  = d.jurisdiction_name,
    is_recueil         = d.is_recueil,
    is_tables_lebon    = d.is_tables_lebon,
    date_lecture       = d.date_lecture,
    legal_instruments  = li.instruments
FROM decisions d
LEFT JOIN LATERAL (
    SELECT ARRAY_AGG(DISTINCT instrument ORDER BY instrument) AS instruments
    FROM decision_legal_references
    WHERE decision_id = d.id
) li ON TRUE
WHERE c.decision_id = d.id;

CREATE INDEX idx_chunks_jurisdiction_level ON decision_chunks (jurisdiction_level);
CREATE INDEX idx_chunks_instance_level     ON decision_chunks (instance_level);
CREATE INDEX idx_chunks_main_outcome       ON decision_chunks (main_outcome);
CREATE INDEX idx_chunks_special_procedure  ON decision_chunks (special_procedure);
CREATE INDEX idx_chunks_jurisdiction_name  ON decision_chunks (LOWER(jurisdiction_name));
CREATE INDEX idx_chunks_is_recueil         ON decision_chunks (is_recueil);
CREATE INDEX idx_chunks_is_tables_lebon    ON decision_chunks (is_tables_lebon);
CREATE INDEX idx_chunks_date_lecture       ON decision_chunks (date_lecture);
CREATE INDEX idx_chunks_legal_instruments  ON decision_chunks USING GIN (legal_instruments);

CREATE OR REPLACE FUNCTION sync_chunks_decision_meta() RETURNS TRIGGER AS $$
BEGIN
    UPDATE decision_chunks SET
        jurisdiction_level = NEW.jurisdiction_level,
        instance_level     = NEW.instance_level,
        main_outcome       = NEW.main_outcome,
        special_procedure  = NEW.special_procedure,
        jurisdiction_name  = NEW.jurisdiction_name,
        is_recueil         = NEW.is_recueil,
        is_tables_lebon    = NEW.is_tables_lebon,
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
     OR OLD.is_recueil         IS DISTINCT FROM NEW.is_recueil
     OR OLD.is_tables_lebon    IS DISTINCT FROM NEW.is_tables_lebon
     OR OLD.date_lecture       IS DISTINCT FROM NEW.date_lecture
    )
    EXECUTE FUNCTION sync_chunks_decision_meta();

CREATE OR REPLACE FUNCTION sync_chunks_legal_instruments() RETURNS TRIGGER AS $$
DECLARE
    target_decision_id BIGINT;
BEGIN
    target_decision_id := COALESCE(NEW.decision_id, OLD.decision_id);
    UPDATE decision_chunks SET legal_instruments = (
        SELECT ARRAY_AGG(DISTINCT instrument ORDER BY instrument)
        FROM decision_legal_references
        WHERE decision_id = target_decision_id
    )
    WHERE decision_id = target_decision_id;
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER dlr_sync_chunks_legal_instruments
    AFTER INSERT OR UPDATE OR DELETE ON decision_legal_references
    FOR EACH ROW
    EXECUTE FUNCTION sync_chunks_legal_instruments();

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
    is_recueil,
    is_tables_lebon,
    date_lecture
)
WITH (
  key_field = 'id',
  text_fields = '{"body": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true}, "record": "position"}}'
);

CREATE INDEX chunks_vec ON decision_chunks
USING vchordrq (embedding rabitq8_cosine_ops);
