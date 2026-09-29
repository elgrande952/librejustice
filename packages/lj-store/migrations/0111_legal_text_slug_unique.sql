UPDATE legal_text t
SET slug = t.slug || '-' || lower(t.text_uid)
WHERE t.slug IS NOT NULL
  AND EXISTS (
    SELECT 1 FROM legal_text o
    WHERE o.slug = t.slug AND o.text_uid < t.text_uid
  );

DROP INDEX IF EXISTS idx_legal_text_slug;
CREATE UNIQUE INDEX idx_legal_text_slug ON legal_text (slug)
WHERE slug IS NOT NULL;
