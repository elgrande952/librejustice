ALTER TABLE legal_citation ADD COLUMN suivants boolean NOT NULL DEFAULT false;

CREATE INDEX idx_legal_toc_edge_art_num
    ON legal_toc_edge (text_uid, child_num_key) WHERE child_kind = 'article';

CREATE FUNCTION _suivants_family_keys(p_uid text, p_num_key text)
RETURNS text[]
LANGUAGE sql STABLE
AS $$
    WITH anchor AS (
        SELECT owner_uid, min(seq) AS seq
        FROM legal_toc_edge
        WHERE text_uid = p_uid AND child_kind = 'article'
          AND child_num_key = p_num_key AND etat = 'VIGUEUR'
        GROUP BY owner_uid
    ),
    members AS (
        SELECT a.owner_uid, e.child_num_key, min(e.seq) AS seq_min
        FROM anchor a
        JOIN legal_toc_edge e
          ON e.owner_uid = a.owner_uid
         AND e.child_kind = 'article' AND e.etat = 'VIGUEUR'
         AND e.seq >= a.seq
        GROUP BY a.owner_uid, e.child_num_key
    ),
    fams AS (
        SELECT owner_uid, array_agg(child_num_key ORDER BY seq_min) AS fam
        FROM members
        GROUP BY owner_uid
    )
    SELECT CASE WHEN count(DISTINCT fam) = 1
                 AND cardinality(min(fam)) BETWEEN 2 AND 20
                THEN min(fam)
           END
    FROM fams;
$$;

CREATE FUNCTION _suivants_family(p_uid text, p_cited text)
RETURNS text[]
LANGUAGE sql STABLE
AS $$
    WITH pub AS (
        SELECT CASE
            WHEN p_cited ~ '^[LRDA]\. ?\d[0-9-]*$'
                THEN lower(regexp_replace(p_cited, '\. ?', ''))
            WHEN p_cited ~ '^\d[0-9-]*$' THEN p_cited
        END AS anchor_key
    ),
    fam AS (
        SELECT unnest(_suivants_family_keys(p_uid, pub.anchor_key)) AS k
        FROM pub WHERE pub.anchor_key IS NOT NULL
    ),
    citable AS (
        SELECT CASE
            WHEN k ~ '^[lrda]\d[0-9-]*$'
                THEN upper(left(k, 1)) || '. ' || substr(k, 2)
            WHEN k ~ '^\d[0-9-]*$' THEN k
        END AS c
        FROM fam
    )
    SELECT CASE WHEN count(c) >= 2 THEN array_agg(c) END
    FROM citable WHERE c IS NOT NULL;
$$;

CREATE OR REPLACE FUNCTION _sync_decisions_legal_instruments_for(p_ids bigint[])
RETURNS bigint
LANGUAGE sql
AS $$
    WITH pairs AS (
        SELECT lc.decision_id, lc.ref_text_uid,
               lc.ref_text_uid || '|' || k AS composite
        FROM legal_citation lc
        LEFT JOIN LATERAL unnest(
            CASE WHEN lc.suivants AND lc.ref_num_key IS NOT NULL
                 THEN coalesce(_suivants_family(lc.ref_text_uid, lc.ref_num_key),
                               ARRAY[lc.ref_num_key])
                 ELSE ARRAY[lc.ref_num_key] END
        ) AS k ON true
        WHERE lc.decision_id = ANY(p_ids)
    ),
    agg AS (
        SELECT
            t.id AS decision_id,
            ARRAY_AGG(DISTINCT p.ref_text_uid COLLATE "C"
                      ORDER BY p.ref_text_uid COLLATE "C")
                FILTER (WHERE p.ref_text_uid IS NOT NULL) AS instruments_arr,
            ARRAY_AGG(DISTINCT p.composite COLLATE "C"
                      ORDER BY p.composite COLLATE "C")
                FILTER (WHERE p.composite IS NOT NULL) AS composite_arr
        FROM unnest(p_ids) AS t(id)
        LEFT JOIN pairs p ON p.decision_id = t.id
        GROUP BY t.id
    ),
    upd AS (
        UPDATE decisions d
        SET legal_instruments       = agg.instruments_arr,
            legal_article_composite = agg.composite_arr
        FROM agg
        WHERE d.id = agg.decision_id
          AND (
                d.legal_instruments       IS DISTINCT FROM agg.instruments_arr
             OR d.legal_article_composite IS DISTINCT FROM agg.composite_arr
          )
        RETURNING 1
    )
    SELECT count(*) FROM upd;
$$;

CREATE OR REPLACE FUNCTION _sync_chunks_legal_instruments_for(p_ids bigint[])
RETURNS bigint
LANGUAGE sql
AS $$
    WITH pairs AS (
        SELECT lc.decision_id, lc.ref_text_uid,
               lc.ref_text_uid || '|' || k AS composite
        FROM legal_citation lc
        LEFT JOIN LATERAL unnest(
            CASE WHEN lc.suivants AND lc.ref_num_key IS NOT NULL
                 THEN coalesce(_suivants_family(lc.ref_text_uid, lc.ref_num_key),
                               ARRAY[lc.ref_num_key])
                 ELSE ARRAY[lc.ref_num_key] END
        ) AS k ON true
        WHERE lc.decision_id = ANY(p_ids)
    ),
    agg AS (
        SELECT
            t.id AS decision_id,
            ARRAY_AGG(DISTINCT p.ref_text_uid COLLATE "C"
                      ORDER BY p.ref_text_uid COLLATE "C")
                FILTER (WHERE p.ref_text_uid IS NOT NULL) AS instruments_arr,
            ARRAY_AGG(DISTINCT p.composite COLLATE "C"
                      ORDER BY p.composite COLLATE "C")
                FILTER (WHERE p.composite IS NOT NULL) AS composite_arr
        FROM unnest(p_ids) AS t(id)
        LEFT JOIN pairs p ON p.decision_id = t.id
        GROUP BY t.id
    ),
    upd AS (
        UPDATE decision_chunks c
        SET legal_instruments       = agg.instruments_arr,
            legal_article_composite = agg.composite_arr
        FROM agg
        WHERE c.decision_id = agg.decision_id
          AND (
                c.legal_instruments       IS DISTINCT FROM agg.instruments_arr
             OR c.legal_article_composite IS DISTINCT FROM agg.composite_arr
          )
        RETURNING 1
    )
    SELECT count(*) FROM upd;
$$;
