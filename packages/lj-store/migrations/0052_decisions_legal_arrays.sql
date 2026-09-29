ALTER TABLE decisions
    ADD COLUMN IF NOT EXISTS legal_instruments       TEXT[],
    ADD COLUMN IF NOT EXISTS legal_article_composite TEXT[];

CREATE OR REPLACE FUNCTION _sync_decisions_legal_instruments_for(p_ids bigint[])
RETURNS void AS $$
    WITH agg AS (
        SELECT
            t.id AS decision_id,
            ARRAY_AGG(DISTINCT COALESCE(canon.name, lc.name)
                      ORDER BY COALESCE(canon.name, lc.name))
                FILTER (WHERE lc.name IS NOT NULL) AS instruments_arr,
            ARRAY_AGG(DISTINCT COALESCE(canon.name, lc.name) || '|' || la.label
                      ORDER BY COALESCE(canon.name, lc.name) || '|' || la.label)
                FILTER (WHERE la.label IS NOT NULL) AS composite_arr
        FROM unnest(p_ids) AS t(id)
        LEFT JOIN decision_legal_references dlr ON dlr.decision_id = t.id
        LEFT JOIN legal_articles la             ON la.id = dlr.article_id
        LEFT JOIN legal_codes    lc             ON lc.id = la.code_id
        LEFT JOIN legal_codes    canon          ON canon.id = lc.canonical_id
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

DO $$
DECLARE
    b     bigint := 0;
    maxid bigint;
    step  bigint := 20000;
BEGIN
    SELECT max(id) INTO maxid FROM decisions;
    WHILE maxid IS NOT NULL AND b < maxid LOOP
        PERFORM _sync_decisions_legal_instruments_for(
            ARRAY(SELECT id FROM decisions WHERE id > b AND id <= b + step)
        );
        b := b + step;
    END LOOP;
END $$;

ANALYZE decisions;
