CREATE OR REPLACE FUNCTION _sync_decisions_legal_instruments_for(p_ids bigint[])
RETURNS void AS $$
    WITH agg AS (
        SELECT
            t.id AS decision_id,
            ARRAY_AGG(DISTINCT cr.text_key ORDER BY cr.text_key)
                FILTER (WHERE cr.text_key IS NOT NULL) AS instruments_arr,
            ARRAY_AGG(DISTINCT cr.text_key || '|' || cr.article_key
                      ORDER BY cr.text_key || '|' || cr.article_key)
                FILTER (WHERE cr.article_key IS NOT NULL) AS composite_arr
        FROM unnest(p_ids) AS t(id)
        LEFT JOIN decision_citation dc ON dc.decision_id = t.id
        LEFT JOIN cited_reference   cr ON cr.id = dc.cited_reference_id
        GROUP BY t.id
    )
    UPDATE decisions d
    SET legal_instruments       = agg.instruments_arr,
        legal_article_composite = agg.composite_arr
    FROM agg
    WHERE d.id = agg.decision_id
      AND (
            d.legal_instruments       IS DISTINCT FROM agg.instruments_arr
         OR d.legal_article_composite IS DISTINCT FROM agg.composite_arr
      );
$$ LANGUAGE sql;

CREATE OR REPLACE FUNCTION _sync_chunks_legal_instruments_for(p_ids bigint[])
RETURNS void AS $$
    WITH agg AS (
        SELECT
            t.id AS decision_id,
            ARRAY_AGG(DISTINCT cr.text_key ORDER BY cr.text_key)
                FILTER (WHERE cr.text_key IS NOT NULL) AS instruments_arr,
            ARRAY_AGG(DISTINCT cr.text_key || '|' || cr.article_key
                      ORDER BY cr.text_key || '|' || cr.article_key)
                FILTER (WHERE cr.article_key IS NOT NULL) AS composite_arr
        FROM unnest(p_ids) AS t(id)
        LEFT JOIN decision_citation dc ON dc.decision_id = t.id
        LEFT JOIN cited_reference   cr ON cr.id = dc.cited_reference_id
        GROUP BY t.id
    )
    UPDATE decision_chunks c
    SET legal_instruments       = agg.instruments_arr,
        legal_article_composite = agg.composite_arr
    FROM agg
    WHERE c.decision_id = agg.decision_id
      AND (
            c.legal_instruments       IS DISTINCT FROM agg.instruments_arr
         OR c.legal_article_composite IS DISTINCT FROM agg.composite_arr
      );
$$ LANGUAGE sql;

CREATE OR REPLACE FUNCTION sync_citation_arrays_ins()
RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    ids bigint[] := ARRAY(SELECT DISTINCT decision_id FROM new_rows);
BEGIN
    PERFORM _sync_decisions_legal_instruments_for(ids);
    PERFORM _sync_chunks_legal_instruments_for(ids);
    RETURN NULL;
END;
$$;

CREATE OR REPLACE FUNCTION sync_citation_arrays_del()
RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    ids bigint[] := ARRAY(SELECT DISTINCT decision_id FROM old_rows);
BEGIN
    PERFORM _sync_decisions_legal_instruments_for(ids);
    PERFORM _sync_chunks_legal_instruments_for(ids);
    RETURN NULL;
END;
$$;

DROP TRIGGER IF EXISTS dlr_sync_chunks_legal_instruments_ins ON decision_legal_references;
DROP TRIGGER IF EXISTS dlr_sync_chunks_legal_instruments_del ON decision_legal_references;
DROP TRIGGER IF EXISTS dlr_sync_chunks_legal_instruments_upd ON decision_legal_references;

CREATE TRIGGER dc_sync_arrays_ins
    AFTER INSERT ON decision_citation
    REFERENCING NEW TABLE AS new_rows
    FOR EACH STATEMENT EXECUTE FUNCTION sync_citation_arrays_ins();

CREATE TRIGGER dc_sync_arrays_del
    AFTER DELETE ON decision_citation
    REFERENCING OLD TABLE AS old_rows
    FOR EACH STATEMENT EXECUTE FUNCTION sync_citation_arrays_del();
