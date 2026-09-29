CREATE INDEX IF NOT EXISTS annonce_titre_trgm_idx
    ON annonce USING gin (titre gin_trgm_ops);

CREATE INDEX IF NOT EXISTS annonce_acteur_libelle_trgm_idx
    ON annonce USING gin (acteur_libelle gin_trgm_ops);
