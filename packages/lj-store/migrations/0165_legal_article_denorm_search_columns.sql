SET LOCAL statement_timeout = 0;

ALTER TABLE legal_article
    ADD COLUMN jurisdiction text,
    ADD COLUMN nature text,
    ADD COLUMN slug text,
    ADD COLUMN searchable boolean;

DROP INDEX legal_article_bm25;

UPDATE legal_article a SET
    jurisdiction = t.jurisdiction,
    nature = upper(t.nature),
    slug = t.slug,
    searchable = (t.slug IS NOT NULL
                  AND t.role NOT IN ('individuel', 'vehicule', 'habilitation'))
FROM legal_text t
WHERE t.text_uid = a.text_uid;

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
    searchable
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
