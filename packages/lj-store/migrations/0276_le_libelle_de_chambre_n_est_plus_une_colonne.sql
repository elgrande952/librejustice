-- Migration 0276. Le libellé est une fonction pure de `chamber_key` et
-- `speciality_uid`, recomposée à la lecture. Le serveur qui lisait la colonne
-- est remplacé depuis le déploiement du 28/08, ses 2 211 lignes étaient vides.

ALTER TABLE chamber DROP COLUMN label;
