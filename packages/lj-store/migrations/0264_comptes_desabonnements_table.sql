CREATE TABLE comptes.desabonnements (
    email     TEXT PRIMARY KEY,
    origine   TEXT NOT NULL,
    ajoute_le TIMESTAMPTZ NOT NULL DEFAULT now()
);
