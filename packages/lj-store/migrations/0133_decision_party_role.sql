ALTER TABLE decision_party ADD COLUMN role text
    CHECK (role IN ('substituant', 'substitue', 'postulant', 'plaidant'));
