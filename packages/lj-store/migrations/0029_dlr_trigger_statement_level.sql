DROP TRIGGER IF EXISTS dlr_sync_chunks_legal_instruments ON decision_legal_references;

CREATE OR REPLACE FUNCTION _sync_chunks_legal_instruments_for(p_ids bigint[])
RETURNS void AS $$
    WITH agg AS (
        SELECT t.id AS decision_id,
               ARRAY_AGG(DISTINCT lc.name ORDER BY lc.name)
                   FILTER (WHERE lc.name IS NOT NULL) AS arr
        FROM unnest(p_ids) AS t(id)
        LEFT JOIN decision_legal_references dlr ON dlr.decision_id = t.id
        LEFT JOIN legal_articles la             ON la.id = dlr.article_id
        LEFT JOIN legal_codes    lc             ON lc.id = la.code_id
        GROUP BY t.id
    )
    UPDATE decision_chunks c
    SET legal_instruments = agg.arr
    FROM agg
    WHERE c.decision_id = agg.decision_id
      AND c.legal_instruments IS DISTINCT FROM agg.arr;
$$ LANGUAGE sql;

CREATE OR REPLACE FUNCTION sync_chunks_legal_instruments_ins() RETURNS TRIGGER AS $$
BEGIN
    PERFORM _sync_chunks_legal_instruments_for(
        ARRAY(SELECT DISTINCT decision_id FROM new_rows)
    );
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION sync_chunks_legal_instruments_del() RETURNS TRIGGER AS $$
BEGIN
    PERFORM _sync_chunks_legal_instruments_for(
        ARRAY(SELECT DISTINCT decision_id FROM old_rows)
    );
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION sync_chunks_legal_instruments_upd() RETURNS TRIGGER AS $$
BEGIN
    PERFORM _sync_chunks_legal_instruments_for(
        ARRAY(
            SELECT decision_id FROM new_rows
            UNION
            SELECT decision_id FROM old_rows
        )
    );
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER dlr_sync_chunks_legal_instruments_ins
    AFTER INSERT ON decision_legal_references
    REFERENCING NEW TABLE AS new_rows
    FOR EACH STATEMENT
    EXECUTE FUNCTION sync_chunks_legal_instruments_ins();

CREATE TRIGGER dlr_sync_chunks_legal_instruments_del
    AFTER DELETE ON decision_legal_references
    REFERENCING OLD TABLE AS old_rows
    FOR EACH STATEMENT
    EXECUTE FUNCTION sync_chunks_legal_instruments_del();

CREATE TRIGGER dlr_sync_chunks_legal_instruments_upd
    AFTER UPDATE ON decision_legal_references
    REFERENCING NEW TABLE AS new_rows OLD TABLE AS old_rows
    FOR EACH STATEMENT
    EXECUTE FUNCTION sync_chunks_legal_instruments_upd();

DROP FUNCTION IF EXISTS sync_chunks_legal_instruments();
