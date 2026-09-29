UPDATE decisions SET jurisdiction_code = 'ta_amiens' WHERE jurisdiction_code = 'ta_d_amiens';
UPDATE decisions SET jurisdiction_code = 'caa_versailles' WHERE jurisdiction_code = 'caa_versailless';
UPDATE decisions SET jurisdiction_code = 'caa_marseille' WHERE jurisdiction_code = 'caa_montpellier';
DELETE FROM jurisdiction WHERE code IN ('ta_d_amiens', 'caa_versailless', 'caa_montpellier');

UPDATE jurisdiction j
SET city = v.city
FROM (VALUES
    ('tj_havre', 'Le Havre'),
    ('tcom_havre', 'Le Havre'),
    ('tj_mans', 'Le Mans'),
    ('tcom_mans', 'Le Mans'),
    ('tcom_puy_en_velay', 'Le Puy-en-Velay'),
    ('tcom_roche_sur_yon', 'La Roche-sur-Yon'),
    ('tj_sables_d_olonne', 'Les Sables-d''Olonne'),
    ('ta_reunion', 'La Réunion'),
    ('ta_st_barthelemy', 'Saint-Barthélemy'),
    ('ta_st_martin', 'Saint-Martin')
) AS v(code, city)
WHERE j.code = v.code;

UPDATE jurisdiction SET label = 'Tribunal administratif de Saint-Barthélemy' WHERE code = 'ta_st_barthelemy';
UPDATE jurisdiction SET label = 'Tribunal administratif de Saint-Martin' WHERE code = 'ta_st_martin';

CREATE TEMP TABLE jurisdiction_rename AS
SELECT code AS old_code,
       lower(juridiction_type) || '_' || trim(both '_' from regexp_replace(
           translate(lower(replace(replace(city, 'œ', 'oe'), 'Œ', 'oe')),
                     'àâäáéèêëíîïóôöúùûüçÿñ', 'aaaaeeeeiiiooouuuucyn'),
           '[^a-z0-9]+', '_', 'g')) AS new_code
FROM jurisdiction
WHERE juridiction_type IN ('TJ', 'TCOM', 'CA', 'CAA', 'TA') AND city IS NOT NULL;
DELETE FROM jurisdiction_rename WHERE new_code = old_code;

ALTER TABLE decisions DROP CONSTRAINT decisions_jurisdiction_code_fkey;

UPDATE jurisdiction j SET code = r.new_code
FROM jurisdiction_rename r WHERE j.code = r.old_code;

UPDATE decisions d SET jurisdiction_code = r.new_code
FROM jurisdiction_rename r WHERE d.jurisdiction_code = r.old_code;

ALTER TABLE decisions ADD CONSTRAINT decisions_jurisdiction_code_fkey
    FOREIGN KEY (jurisdiction_code) REFERENCES jurisdiction(code);

UPDATE user_search_history h
SET filters = jsonb_set(h.filters, '{jurisdictionCode}', (
        SELECT jsonb_agg(coalesce(m.new_code, x.v))
        FROM jsonb_array_elements_text(h.filters -> 'jurisdictionCode') AS x(v)
        LEFT JOIN (
            SELECT old_code, new_code FROM jurisdiction_rename
            UNION ALL
            VALUES ('ta_d_amiens', 'ta_amiens'),
                   ('caa_versailless', 'caa_versailles'),
                   ('caa_montpellier', 'caa_marseille')
        ) m ON m.old_code = x.v))
WHERE h.filters ? 'jurisdictionCode';

DROP TABLE jurisdiction_rename;
