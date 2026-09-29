DROP INDEX IF EXISTS legal_article_bm25;

ALTER TABLE legal_article ADD COLUMN IF NOT EXISTS code_title TEXT;

ALTER TABLE legal_article ADD COLUMN IF NOT EXISTS search_title TEXT GENERATED ALWAYS AS (
  COALESCE(code_title, '')
  || ', ' || num
  || CASE WHEN title_path IS NOT NULL AND title_path <> ''
       THEN ', ' || regexp_replace(title_path, '^.*>\s*', '')
       ELSE '' END
) STORED;

UPDATE legal_article a
SET code_title = t.title
FROM legal_text t
WHERE a.text_uid = t.text_uid
  AND a.code_title IS DISTINCT FROM t.title;

SET LOCAL maintenance_work_mem = '2GB';
SET LOCAL max_parallel_maintenance_workers = 4;

CREATE INDEX IF NOT EXISTS legal_article_bm25 ON legal_article
USING bm25 (
    id,
    search_title,
    texte,
    num,
    (source::pdb.literal),
    (text_uid::pdb.literal),
    (status::pdb.literal)
)
WITH (
  key_field = 'id',
  text_fields = '{
    "search_title": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "freq"},
    "texte":        {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "freq"},
    "num":          {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true}, "record": "basic"}
  }'
);

ANALYZE legal_article;
