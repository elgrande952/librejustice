-- lj:sans-transaction
CREATE INDEX CONCURRENTLY annonce_bm25 ON annonce
USING bm25 (uid, titre, acteur_libelle, fond, famille_uid, cpv_uid, departement)
WITH (
  key_field = 'uid',
  text_fields = '{
    "titre":          {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"},
    "acteur_libelle": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"},
    "fond":           {"tokenizer": {"type": "raw"}, "fast": true},
    "famille_uid":    {"tokenizer": {"type": "raw"}, "fast": true},
    "cpv_uid":        {"tokenizer": {"type": "raw"}, "fast": true},
    "departement":    {"tokenizer": {"type": "raw"}, "fast": true}
  }'
);
