SET LOCAL maintenance_work_mem = '2GB';
SET LOCAL max_parallel_maintenance_workers = 4;

ALTER TABLE decision_chunks
    ADD COLUMN IF NOT EXISTS legal_article_composite TEXT[];

CREATE INDEX IF NOT EXISTS idx_chunks_legal_article_composite
    ON decision_chunks USING GIN (legal_article_composite);

UPDATE decision_chunks c
SET legal_article_composite = comp_agg.composite
FROM (
    SELECT
        dlr.decision_id,
        ARRAY_AGG(DISTINCT lc.name || '|' || la.label
                  ORDER BY lc.name || '|' || la.label)
            FILTER (WHERE la.label IS NOT NULL) AS composite
    FROM decision_legal_references dlr
    JOIN legal_articles la ON la.id = dlr.article_id
    JOIN legal_codes    lc ON lc.id = la.code_id
    GROUP BY dlr.decision_id
) comp_agg
WHERE c.decision_id = comp_agg.decision_id
  AND c.legal_article_composite IS DISTINCT FROM comp_agg.composite;

CREATE OR REPLACE FUNCTION _sync_chunks_legal_instruments_for(p_ids bigint[])
RETURNS void AS $$
    WITH agg AS (
        SELECT
            t.id AS decision_id,
            ARRAY_AGG(DISTINCT lc.name ORDER BY lc.name)
                FILTER (WHERE lc.name IS NOT NULL) AS instruments_arr,
            ARRAY_AGG(DISTINCT la.label ORDER BY la.label)
                FILTER (WHERE la.label IS NOT NULL) AS labels_arr,
            ARRAY_AGG(DISTINCT lc.name || '|' || la.label
                      ORDER BY lc.name || '|' || la.label)
                FILTER (WHERE la.label IS NOT NULL) AS composite_arr
        FROM unnest(p_ids) AS t(id)
        LEFT JOIN decision_legal_references dlr ON dlr.decision_id = t.id
        LEFT JOIN legal_articles la             ON la.id = dlr.article_id
        LEFT JOIN legal_codes    lc             ON lc.id = la.code_id
        GROUP BY t.id
    )
    UPDATE decision_chunks c
    SET legal_instruments        = agg.instruments_arr,
        legal_article_labels     = agg.labels_arr,
        legal_article_composite  = agg.composite_arr
    FROM agg
    WHERE c.decision_id = agg.decision_id
      AND (
            c.legal_instruments       IS DISTINCT FROM agg.instruments_arr
         OR c.legal_article_labels    IS DISTINCT FROM agg.labels_arr
         OR c.legal_article_composite IS DISTINCT FROM agg.composite_arr
      );
$$ LANGUAGE sql;
