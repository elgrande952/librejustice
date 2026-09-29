ALTER TABLE legal_text ADD COLUMN curated boolean NOT NULL DEFAULT false;

UPDATE legal_text t
SET curated = true
WHERE t.text_uid NOT LIKE 'BOI-%'
  AND EXISTS (
    SELECT 1 FROM legal_article a
    WHERE a.text_uid = t.text_uid
      AND a.source_uid LIKE t.text_uid || '#%');
