-- Migration 0231 : compteurs de facettes du fonds entier.
--
-- Le mode parcours (`/decisions` sans requête) affiche total et facettes du
-- fonds sans filtre : 6,95 s d'agrégats à la demande pour un état qui ne change
-- qu'une fois par nuit, contre 644 lignes à lire ici. Le recompte, lui, coûte
-- 11,6 s.
--
-- `value` porte la valeur BRUTE de la colonne, jamais une clé de référentiel :
-- la publication a changé de regroupement deux fois en un jour (0229, 0230), et
-- un fait brut ne se périme pas quand la correspondance bouge. D'où la dimension
-- `publication_codes`, dont le lecteur dérive les clés comme sur le chemin
-- filtré.
--
-- Écrivain unique : l'étape `resync-decision-facet-counts`, quotidienne.

CREATE TABLE decision_facet_counts (
    dimension text   NOT NULL,
    value     text   NOT NULL,
    parent    text,
    n         bigint NOT NULL,
    CONSTRAINT decision_facet_counts_key
        UNIQUE NULLS NOT DISTINCT (dimension, value, parent)
);

INSERT INTO decision_facet_counts (dimension, value, parent, n)
    SELECT 'total'::text, ''::text, NULL::text, count(*)
    FROM decisions WHERE deleted_at IS NULL
UNION ALL
    SELECT 'jurisdiction_type', jurisdiction_type, NULL, count(*)
    FROM decisions WHERE deleted_at IS NULL
    GROUP BY jurisdiction_type
UNION ALL
    SELECT 'jurisdiction_code', jurisdiction_code, jurisdiction_type, count(*)
    FROM decisions WHERE deleted_at IS NULL AND jurisdiction_code IS NOT NULL
    GROUP BY jurisdiction_code, jurisdiction_type
UNION ALL
    SELECT 'chamber', chamber_uid, NULL, count(*)
    FROM decisions WHERE deleted_at IS NULL AND chamber_uid IS NOT NULL
    GROUP BY chamber_uid
UNION ALL
    SELECT 'office', office_uid, NULL, count(*)
    FROM decisions WHERE deleted_at IS NULL AND office_uid IS NOT NULL
    GROUP BY office_uid
UNION ALL
    SELECT 'legal_domain', legal_domain_uid, NULL, count(*)
    FROM decisions WHERE deleted_at IS NULL AND legal_domain_uid IS NOT NULL
    GROUP BY legal_domain_uid
UNION ALL
    SELECT 'solution', solution_uid, NULL, count(*)
    FROM decisions WHERE deleted_at IS NULL AND solution_uid IS NOT NULL
    GROUP BY solution_uid
UNION ALL
    SELECT 'year', extract(year FROM date_lecture)::int::text, NULL, count(*)
    FROM decisions WHERE deleted_at IS NULL AND date_lecture IS NOT NULL
    GROUP BY 2
UNION ALL
    SELECT 'publication_codes', array_to_string(publication_codes, ','), NULL, count(*)
    FROM decisions WHERE deleted_at IS NULL AND publication_codes <> '{}'
    GROUP BY 2;
