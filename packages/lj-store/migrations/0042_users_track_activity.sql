-- Migration 0042 : préférence « enregistrer mon activité » (mode ZDR)
-- L'utilisateur peut couper tout enregistrement
-- (recherches, lectures, signets). 'true' par défaut — comportement actuel.

ALTER TABLE users
    ADD COLUMN track_activity BOOLEAN NOT NULL DEFAULT true;
