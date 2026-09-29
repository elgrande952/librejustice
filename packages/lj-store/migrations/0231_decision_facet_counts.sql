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
