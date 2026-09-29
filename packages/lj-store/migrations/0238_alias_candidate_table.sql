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
