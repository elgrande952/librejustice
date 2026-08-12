-- La famille « et suivants » se lit dans le TOC à la date de la
-- décision, plus dans celui du jour. Le filtre `etat = 'VIGUEUR'` cède la place
-- à la fenêtre de validité de l'arête ; les garde-fous (section
-- unique, cap 2..20, NULL sinon) ne changent pas.

CREATE OR REPLACE FUNCTION public._suivants_family_keys(
    p_uid text, p_num_key text, p_date date
) RETURNS text[]
LANGUAGE sql
STABLE
AS $function$
    WITH anchor AS (
        SELECT owner_uid, min(seq) AS seq
        FROM legal_toc_edge
        WHERE text_uid = p_uid AND child_kind = 'article'
          AND child_num_key = p_num_key
          -- Arête en vigueur à la date visée. `date_debut` nul = de toujours
          -- (210 k arêtes VIGUEUR sans borne basse) : conservateur, elle reste
          -- dans la famille.
          AND (date_debut IS NULL OR date_debut <= p_date)
          AND (date_fin   IS NULL OR date_fin   >  p_date)
        GROUP BY owner_uid
    ),
    members AS (
        SELECT a.owner_uid, e.child_num_key, min(e.seq) AS seq_min
        FROM anchor a
        JOIN legal_toc_edge e
          ON e.owner_uid = a.owner_uid
         AND e.child_kind = 'article'
         AND e.seq >= a.seq
         AND (e.date_debut IS NULL OR e.date_debut <= p_date)
         AND (e.date_fin   IS NULL OR e.date_fin   >  p_date)
        GROUP BY a.owner_uid, e.child_num_key
    ),
    -- Ordre de LECTURE (seq TOC), pas lexicographique (« l3213-10 » suivrait
    -- « l3213-1 » et précéderait « l3213-2 » sinon).
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
$function$;

-- Les deux resyncs passent la date de lecture de la décision citante.
CREATE OR REPLACE FUNCTION public._sync_decisions_legal_instruments_for(p_ids bigint[])
RETURNS bigint
LANGUAGE sql
AS $function$
    WITH pairs AS (
        SELECT lc.decision_id, el->>2 AS ref_text_uid,
               (el->>2) || '|' || k AS composite
        FROM legal_citation lc
        JOIN decisions d0 ON d0.id = lc.decision_id
        CROSS JOIN LATERAL jsonb_array_elements(lc.spans) AS el
        LEFT JOIN LATERAL unnest(
            CASE WHEN (el->>4)::bool AND el->>3 IS NOT NULL
                 THEN coalesce(_suivants_family_keys(el->>2, el->>3,
                                   coalesce(d0.date_lecture, CURRENT_DATE)),
                               ARRAY[el->>3])
                 ELSE ARRAY[el->>3] END
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
$function$;

CREATE OR REPLACE FUNCTION public._sync_chunks_legal_instruments_for(p_ids bigint[])
RETURNS bigint
LANGUAGE sql
AS $function$
    WITH pairs AS (
        SELECT lc.decision_id, el->>2 AS ref_text_uid,
               (el->>2) || '|' || k AS composite
        FROM legal_citation lc
        JOIN decisions d0 ON d0.id = lc.decision_id
        CROSS JOIN LATERAL jsonb_array_elements(lc.spans) AS el
        LEFT JOIN LATERAL unnest(
            CASE WHEN (el->>4)::bool AND el->>3 IS NOT NULL
                 THEN coalesce(_suivants_family_keys(el->>2, el->>3,
                                   coalesce(d0.date_lecture, CURRENT_DATE)),
                               ARRAY[el->>3])
                 ELSE ARRAY[el->>3] END
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
$function$;

-- Un seul chemin : la signature sans date disparaît.
DROP FUNCTION public._suivants_family_keys(text, text);
