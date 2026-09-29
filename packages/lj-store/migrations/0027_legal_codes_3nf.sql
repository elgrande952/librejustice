SET LOCAL maintenance_work_mem = '2GB';
SET LOCAL max_parallel_maintenance_workers = 4;

CREATE TABLE legal_codes (
    id   BIGSERIAL PRIMARY KEY,
    name TEXT NOT NULL,
    UNIQUE (name)
);

CREATE TABLE legal_articles (
    id      BIGSERIAL PRIMARY KEY,
    code_id BIGINT NOT NULL REFERENCES legal_codes(id) ON DELETE CASCADE,
    label   TEXT,
    UNIQUE NULLS NOT DISTINCT (code_id, label)
);
CREATE INDEX idx_legal_articles_code ON legal_articles(code_id);

INSERT INTO legal_codes (name)
SELECT DISTINCT instrument
FROM decision_legal_references;

INSERT INTO legal_articles (code_id, label)
SELECT DISTINCT lc.id, dlr.article
FROM decision_legal_references dlr
JOIN legal_codes lc ON lc.name = dlr.instrument;

CREATE TABLE decision_legal_references_v2 (
    decision_id BIGINT NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    article_id  BIGINT NOT NULL REFERENCES legal_articles(id),
    PRIMARY KEY (decision_id, article_id)
);

INSERT INTO decision_legal_references_v2 (decision_id, article_id)
SELECT DISTINCT dlr.decision_id, la.id
FROM decision_legal_references dlr
JOIN legal_codes    lc ON lc.name    = dlr.instrument
JOIN legal_articles la ON la.code_id = lc.id
                       AND la.label  = dlr.article;

DROP TABLE decision_legal_references CASCADE;
ALTER TABLE decision_legal_references_v2 RENAME TO decision_legal_references;
ALTER INDEX decision_legal_references_v2_pkey RENAME TO decision_legal_references_pkey;

CREATE INDEX idx_dlr_article_id ON decision_legal_references(article_id);

CREATE OR REPLACE FUNCTION sync_chunks_legal_instruments() RETURNS TRIGGER AS $$
DECLARE
    target_decision_id BIGINT;
BEGIN
    target_decision_id := COALESCE(NEW.decision_id, OLD.decision_id);
    UPDATE decision_chunks SET legal_instruments = (
        SELECT ARRAY_AGG(DISTINCT lc.name ORDER BY lc.name)
        FROM decision_legal_references dlr
        JOIN legal_articles la ON la.id = dlr.article_id
        JOIN legal_codes    lc ON lc.id = la.code_id
        WHERE dlr.decision_id = target_decision_id
    )
    WHERE decision_id = target_decision_id;
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER dlr_sync_chunks_legal_instruments
    AFTER INSERT OR UPDATE OR DELETE ON decision_legal_references
    FOR EACH ROW
    EXECUTE FUNCTION sync_chunks_legal_instruments();
