-- lj:sans-transaction
CREATE INDEX CONCURRENTLY legal_article_bm25_v2 ON legal_article
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
    "search_title": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "freq"},
    "texte":        {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "freq"},
    "num":          {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true}, "record": "basic"}
  }'
);
