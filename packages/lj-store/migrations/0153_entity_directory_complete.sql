ALTER TABLE entity
    ADD COLUMN category text,
    ADD COLUMN ape text,
    ADD COLUMN barreau text,
    ADD COLUMN decision_count bigint NOT NULL DEFAULT 0;

UPDATE entity SET
    category = CASE
        WHEN uid LIKE 'siren:%' AND nature = 'morale_privee' THEN 'entreprises'
        WHEN uid LIKE 'siren:%' AND nature = 'morale_publique' THEN 'personnes_publiques'
        WHEN uid LIKE 'rna:%' THEN 'associations'
        WHEN uid LIKE 'oacc:firm-%' THEN 'cabinets'
        WHEN uid LIKE 'cnb:%' OR uid LIKE 'oacc:%' THEN 'avocats'
    END,
    barreau = CASE WHEN uid LIKE 'cnb:%' THEN split_part(uid, ':', 2) END;

ALTER TABLE entity ALTER COLUMN category SET NOT NULL;

UPDATE entity e
SET decision_count = c.n
FROM (
    SELECT p.entity_uid, count(DISTINCT p.decision_id) AS n
    FROM decision_party p
    JOIN decisions d ON d.id = p.decision_id
    WHERE p.entity_uid IS NOT NULL AND d.deleted_at IS NULL
    GROUP BY p.entity_uid
) c
WHERE e.uid = c.entity_uid;

CREATE INDEX entity_annuaire_idx
    ON entity (category, decision_count DESC, denomination_folded);

CREATE INDEX entity_barreau_idx
    ON entity (barreau, decision_count DESC, denomination_folded)
    WHERE barreau IS NOT NULL;

CREATE INDEX entity_denomination_prefix_idx
    ON entity (denomination_folded text_pattern_ops);
DROP INDEX entity_denomination_folded_idx;

ALTER TABLE entity SET (fillfactor = 90);

ALTER TABLE annuaire_registre ADD COLUMN contentieux bigint NOT NULL DEFAULT 0;
TRUNCATE annuaire_registre;
INSERT INTO annuaire_registre (category, total, contentieux)
SELECT category, count(*), count(*) FILTER (WHERE decision_count > 0)
FROM entity
GROUP BY category;

DROP TABLE entity_contentieux;
