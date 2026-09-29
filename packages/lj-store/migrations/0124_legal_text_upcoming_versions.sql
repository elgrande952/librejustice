ALTER TABLE legal_text
    ADD COLUMN upcoming_versions date[] NOT NULL DEFAULT '{}';
