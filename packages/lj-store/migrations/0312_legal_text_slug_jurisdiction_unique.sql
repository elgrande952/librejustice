-- lj:sans-transaction
CREATE UNIQUE INDEX CONCURRENTLY idx_legal_text_slug_jurisdiction
ON legal_text (slug, jurisdiction)
WHERE slug IS NOT NULL;
