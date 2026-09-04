-- Migration 0234 : le siège se stocke par sa clé (`CH:2`, `P:1-11`, `SSR:1-4`),
-- plus par son rendu. Renseignée par la ré-extraction, pas ici.
--
-- Additive : `migrate` tourne avec l'ancien serveur debout, qui lit encore
-- `chamber_position`. Le DROP part au déploiement suivant.

ALTER TABLE decisions ADD COLUMN chamber_key TEXT;

CREATE INDEX idx_decisions_chamber_key ON decisions (jurisdiction_code, chamber_key)
  WHERE deleted_at IS NULL AND chamber_key IS NOT NULL;
