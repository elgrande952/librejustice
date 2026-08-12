-- Registre d'avocats vivant : évidences d'activité datées sur
-- `entity` + arêtes d'affiliation avocat↔structure.
--
-- `activity` : tableau d'évidences `[{du?, au?, src}]` — périodes déclarées
-- par la source (avBarEntree CNB, périodes EI SIRENE) ou observées par le
-- rejeu des snapshots. Évidences positives : l'absence n'est pas une
-- inactivité, un span sans `du` est un début inconnu (baseline).

ALTER TABLE entity ADD COLUMN activity jsonb;

-- Actif à la date `d` ? Actif si AU MOINS UN span
-- est compatible avec `d` — une borne absente est un inconnu compatible
-- (un span sans `du` n'exclut jamais vers le passé). L'exclusion n'arrive
-- que quand toutes les évidences contredisent positivement la date (parti
-- avant, entré après — y compris le trou d'un aller-retour). Sans évidence
-- (NULL, tableau vide) ou sans date de décision → permissif (résolution
-- courante).
CREATE FUNCTION lj_active_at(activity jsonb, d date) RETURNS boolean
LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
    SELECT activity IS NULL OR d IS NULL OR COALESCE(
        (SELECT bool_or(
                (NOT s ? 'du' OR (s->>'du')::date <= d)
            AND (NOT s ? 'au' OR (s->>'au')::date >= d))
         FROM jsonb_array_elements(activity) s),
        true)
$$;

-- Affiliations datées avocat ↔ structure d'exercice : arêtes
-- N par avocat, une source d'évidence par arête, sans fusion destructive.
-- Références DOUCES des deux côtés (comme decision_party.entity_uid) : les
-- reloads de registres remplacent des namespaces entiers, une FK les
-- cascaderait. Pas de PK naturelle : un retour dans une structure quittée
-- (A→B→A) est un fait légitime = deux arêtes closes distinctes ;
-- l'idempotence vit chez l'écrivain, qui tient AU PLUS UNE arête ouverte
-- par (entity_uid, source) — index unique partiel.
CREATE TABLE entity_affiliation (
    entity_uid    text NOT NULL,
    structure_uid text NOT NULL,
    du            date,
    au            date,
    source        text NOT NULL
);

CREATE UNIQUE INDEX entity_affiliation_open_idx
    ON entity_affiliation (entity_uid, source) WHERE au IS NULL;
CREATE INDEX entity_affiliation_entity_idx ON entity_affiliation (entity_uid);
CREATE INDEX entity_affiliation_structure_idx ON entity_affiliation (structure_uid);
