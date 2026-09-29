DROP INDEX IF EXISTS legal_article_bm25;

ALTER TABLE legal_article DROP COLUMN IF EXISTS search_title;

ALTER TABLE legal_article ADD COLUMN search_title TEXT GENERATED ALWAYS AS (
  COALESCE(code_title, '')
  || ', ' || num
  || CASE WHEN num LIKE '%*%' THEN ', ' || num_key ELSE '' END
  || CASE WHEN title_path IS NOT NULL AND title_path <> ''
       THEN ', ' || regexp_replace(title_path, '^.*>\s*', '')
       ELSE '' END
) STORED;

SET LOCAL maintenance_work_mem = '2GB';
SET LOCAL max_parallel_maintenance_workers = 4;

CREATE INDEX legal_article_bm25 ON legal_article
USING bm25 (
    id,
    search_title,
    texte,
    num,
    (source::pdb.literal),
    (text_uid::pdb.literal),
    (status::pdb.literal),
    (jurisdiction::pdb.literal),
    (nature::pdb.literal),
    (slug::pdb.literal),
    (kind::pdb.literal),
    searchable,
    date_debut,
    date_fin
)
WITH (
  key_field = 'id',
  text_fields = '{
    "search_title": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "freq"},
    "texte":        {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "freq"},
    "num":          {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true}, "record": "basic"}
  }'
);

ANALYZE legal_article;
