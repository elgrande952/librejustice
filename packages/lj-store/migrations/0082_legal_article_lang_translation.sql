ALTER TABLE legal_article
    ADD COLUMN IF NOT EXISTS texte_original TEXT,
    ADD COLUMN IF NOT EXISTS lang_original  TEXT,
    ADD COLUMN IF NOT EXISTS translation    TEXT NOT NULL DEFAULT 'non_officiel'
        CHECK (translation IN ('officiel', 'non_officiel', 'automatique'));

UPDATE legal_article SET translation = 'officiel'
    WHERE source <> 'jafbase';
UPDATE legal_article SET translation = 'officiel'
    WHERE source = 'jafbase' AND nota ILIKE '%texte officiel%';
