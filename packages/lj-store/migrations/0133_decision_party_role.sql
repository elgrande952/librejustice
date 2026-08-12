-- Rôle explicite des avocats (counsel_name), capté en apposition
-- par l'extracteur. NULL = aucun marqueur dans le texte.
ALTER TABLE decision_party ADD COLUMN role text
    CHECK (role IN ('substituant', 'substitue', 'postulant', 'plaidant'));
