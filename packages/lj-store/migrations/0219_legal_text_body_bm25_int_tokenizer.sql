-- lj:sans-transaction
CREATE INDEX CONCURRENTLY legal_text_body_bm25_v2 ON legal_text
USING bm25 (
  id,
  title,
  body,
  ((text_uid)::pdb.literal),
  ((nature)::pdb.literal),
  ((jurisdiction)::pdb.literal),
  ((slug)::pdb.literal),
  ((role)::pdb.literal),
  ((COALESCE(status, 'VIGUEUR'::text))::pdb.literal('alias=status_eff')),
  ((upper(nature))::pdb.literal('alias=nature_up')),
  ((lower(nature))::pdb.literal('alias=source')),
  (((body IS NOT NULL))::pdb.alias('alias=has_body'))
) WITH (key_field=id, text_fields='{
    "title": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"},
    "body":  {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"}
  }');
