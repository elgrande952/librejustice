ALTER TABLE legal_codes
    ADD COLUMN IF NOT EXISTS canonical_id BIGINT REFERENCES legal_codes(id);

ALTER TABLE legal_codes
    ADD CONSTRAINT legal_codes_canonical_not_self CHECK (canonical_id <> id);

CREATE INDEX IF NOT EXISTS idx_legal_codes_canonical
    ON legal_codes (canonical_id) WHERE canonical_id IS NOT NULL;

CREATE OR REPLACE FUNCTION _sync_chunks_legal_instruments_for(p_ids bigint[])
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
