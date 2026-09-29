CREATE FUNCTION lj_fold(text) RETURNS text
LANGUAGE sql IMMUTABLE PARALLEL SAFE STRICT
AS $fn$
SELECT btrim(regexp_replace(
    lower(translate($1,
        'ÀÂÄàâäÉÈÊËéèêëÎÏîïÔÖôöÙÛÜùûüÇçŒœ'
        || U&'\2019\0009\000A\000B\000C\000D\0085\00A0\1680\2000\2001\2002\2003\2004\2005\2006\2007\2008\2009\200A\2028\2029\202F\205F\3000',
        'aaaaaaeeeeeeeeiiiioooouuuuuuccoo'
        || '''' || '                        ')),
    '\s+', ' ', 'g'))
$fn$;

CREATE FUNCTION lj_fold_all(jsonb) RETURNS text[]
LANGUAGE sql IMMUTABLE PARALLEL SAFE STRICT
AS $fn$
SELECT coalesce(array_agg(public.lj_fold(e->>'d')), '{}')
FROM jsonb_array_elements($1) AS e
$fn$;

DO $$
DECLARE bad bigint;
BEGIN
    SELECT count(*) INTO bad FROM entity
    WHERE lj_fold(denomination) IS DISTINCT FROM denomination_folded;
    IF bad > 0 THEN
        RAISE EXCEPTION 'lj_fold diverge de canon() sur % lignes entity', bad;
    END IF;
    SELECT count(*) INTO bad FROM entity_denomination
    WHERE lj_fold(denomination) IS DISTINCT FROM folded;
    IF bad > 0 THEN
        RAISE EXCEPTION 'lj_fold diverge de canon() sur % lignes entity_denomination', bad;
    END IF;
END $$;

ALTER TABLE entity ADD COLUMN denominations jsonb;

UPDATE entity e SET denominations = d.arr
FROM (
    SELECT entity_uid,
           jsonb_agg(DISTINCT jsonb_strip_nulls(jsonb_build_object(
               'd', denomination,
               'du', date_debut::text,
               'au', date_fin::text))) AS arr
    FROM entity_denomination
    GROUP BY entity_uid
) d
WHERE d.entity_uid = e.uid;

UPDATE entity SET denominations = jsonb_build_array(jsonb_build_object('d', denomination))
WHERE denominations IS NULL;

ALTER TABLE entity ALTER COLUMN denominations SET NOT NULL;

DROP INDEX entity_annuaire_idx;
DROP INDEX entity_denomination_prefix_idx;
DROP INDEX entity_denomination_contentieux_idx;
ALTER TABLE entity DROP COLUMN denomination_folded;

CREATE INDEX entity_annuaire_idx
    ON entity (category, decision_count DESC, lj_fold(denomination));
CREATE INDEX entity_prefix_idx
    ON entity (lj_fold(denomination) text_pattern_ops);
CREATE INDEX entity_prefix_contentieux_idx
    ON entity (lj_fold(denomination) text_pattern_ops)
    WHERE decision_count > 0;
CREATE INDEX entity_folded_all_idx
    ON entity USING gin (lj_fold_all(denominations));

DROP TABLE entity_denomination;
