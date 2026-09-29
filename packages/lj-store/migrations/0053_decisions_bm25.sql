SET LOCAL maintenance_work_mem = '2GB';
SET LOCAL max_parallel_maintenance_workers = 4;

CREATE INDEX decisions_bm25 ON decisions
USING bm25 (
    id,
    full_text,
    search_title,
    (juridiction_type::pdb.literal),
    (jurisdiction_level::pdb.literal),
    (instance_level::pdb.literal),
    (main_outcome::pdb.literal),
    (special_procedure::pdb.literal),
    (jurisdiction_name::pdb.literal),
    (legal_instruments::pdb.literal),
    (legal_article_composite::pdb.literal),
    (publication_codes::pdb.literal),
    date_lecture
)
WITH (
  key_field = 'id',
  text_fields = '{
    "full_text":    {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"},
    "search_title": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"}
  }'
);

ANALYZE decisions;
