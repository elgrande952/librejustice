ALTER TABLE assertion ADD COLUMN rule text;
ALTER TABLE assertion DROP CONSTRAINT assertion_places_check;

ALTER TABLE assertion ADD CONSTRAINT assertion_rule_or_places
    CHECK ((rule IS NULL) = (cardinality(places) > 0)) NOT VALID;

CREATE TABLE coverage_rule (
    rule text NOT NULL,
    place text NOT NULL,
    since date,
    until date,
    since_source text NOT NULL,
    until_source text NOT NULL,
    CONSTRAINT coverage_rule_window CHECK (since IS NULL OR until IS NULL OR until > since)
);
CREATE INDEX coverage_rule_rule_idx ON coverage_rule (rule);

ALTER TABLE text_place_coverage ADD COLUMN rule text;
ALTER TABLE text_place_coverage ALTER COLUMN place DROP NOT NULL;
ALTER TABLE text_place_coverage ADD CONSTRAINT text_place_coverage_place_or_rule
    CHECK ((place IS NULL) <> (rule IS NULL));
CREATE INDEX text_place_coverage_rule_idx
    ON text_place_coverage (rule, journal_code, nature, starts_on, ends_on)
    WHERE rule IS NOT NULL;

CREATE OR REPLACE VIEW assertion_by_place AS
SELECT a.id, a.text_uid, a.num_key, p.place, a.starts_on, a.ends_on,
       a.start_origin, a.end_origin, a.version_start, a.start_source, a.end_source
FROM assertion a, unnest(a.places) AS p(place)
UNION ALL
SELECT k.id, k.text_uid, k.num_key, k.place, k.starts_on, k.ends_on,
       k.start_origin, k.end_origin, k.version_start, k.start_source, k.end_source
FROM (
    SELECT a.id, a.text_uid, a.num_key, r.place, a.version_start,
           CASE WHEN r.since > a.starts_on THEN r.since ELSE a.starts_on END AS starts_on,
           CASE WHEN r.since > a.starts_on THEN 'received' ELSE a.start_origin END AS start_origin,
           CASE WHEN r.since > a.starts_on THEN r.since_source ELSE a.start_source END AS start_source,
           CASE WHEN r.until < coalesce(a.ends_on, 'infinity') THEN r.until ELSE a.ends_on END AS ends_on,
           CASE WHEN r.until < coalesce(a.ends_on, 'infinity') THEN 'received' ELSE a.end_origin END AS end_origin,
           CASE WHEN r.until < coalesce(a.ends_on, 'infinity') THEN r.until_source ELSE a.end_source END AS end_source
    FROM assertion a
    JOIN coverage_rule r ON r.rule = a.rule
) k
WHERE k.ends_on IS NULL OR k.ends_on > k.starts_on;

UPDATE legal_text SET default_scope = 'geo:YT'
WHERE text_uid IN ('LEGITEXT000006072052', 'LEGITEXT000006071645', 'LEGITEXT000006071556',
                   'LEGITEXT000006069472', 'LEGITEXT000006074235');
UPDATE legal_text SET default_scope = 'geo:NC'
WHERE text_uid IN ('LEGITEXT000006070300', 'JORFTEXT000000280786', 'JORFTEXT000000647908');
