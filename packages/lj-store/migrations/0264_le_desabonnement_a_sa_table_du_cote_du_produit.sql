-- Migration 0264 : les adresses qui ne veulent plus recevoir de courrier, que
-- le lien de désabonnement servi par `lj-server` inscrit.

CREATE TABLE comptes.desabonnements (
    email     TEXT PRIMARY KEY,
    origine   TEXT NOT NULL,
    ajoute_le TIMESTAMPTZ NOT NULL DEFAULT now()
);
