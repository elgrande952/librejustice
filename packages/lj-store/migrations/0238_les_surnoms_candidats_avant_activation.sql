-- Migration 0238 : les surnoms candidats, avant leur activation à la main.
--
-- Table reconstructible et jetable : rien en aval n'en dépend, la recherche ne
-- lit que le dataset versionné. Le gisement entre dans la clé pour qu'un
-- gisement se rejoue sans toucher l'autre.
--
-- La cible est `target_ref` plus `target_date`, jamais le seul numéro : 66145
-- rend Cadot (1889) et une décision de 1988. Un renvoi ancien ne donne que la
-- date, d'où `NULLS NOT DISTINCT` sur la clé.

CREATE TABLE alias_candidate (
    genre       text        NOT NULL,
    gisement    text        NOT NULL,
    alias_key   text        NOT NULL,
    alias_vu    text        NOT NULL,
    target_ref  text        NOT NULL,
    target_date date,
    support     integer     NOT NULL,
    calcule_le  timestamptz NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX alias_candidate_cle
    ON alias_candidate (genre, gisement, alias_key, target_ref, target_date)
    NULLS NOT DISTINCT;

CREATE INDEX alias_candidate_revue ON alias_candidate (genre, support DESC);
