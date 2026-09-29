SET LOCAL maintenance_work_mem = '2GB';
SET LOCAL max_parallel_maintenance_workers = 4;

DROP INDEX IF EXISTS chunks_bm25;

CREATE INDEX chunks_bm25 ON decision_chunks
USING bm25 (
    id,
    body,
    (juridiction_type::pdb.literal),
    (jurisdiction_level::pdb.literal),
    (instance_level::pdb.literal),
    (main_outcome::pdb.literal),
    (special_procedure::pdb.literal),
    (jurisdiction_name::pdb.literal),
    (legal_instruments::pdb.literal),
    (legal_article_labels::pdb.literal),
    (publication_codes::pdb.literal),
    date_lecture
)
WITH (
  key_field = 'id',
  text_fields = '{"body": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true}, "record": "position"}}'
);
