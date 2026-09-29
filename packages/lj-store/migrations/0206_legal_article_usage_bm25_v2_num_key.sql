-- lj:sans-transaction
CREATE INDEX CONCURRENTLY legal_article_usage_bm25_v2 ON legal_article_usage
USING bm25 (
  id,
  terms,
  ((text_uid)::pdb.literal),
  ((num_key)::pdb.literal)
) WITH (key_field=id, text_fields='{
    "terms": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}_-]+", "ascii_folding": true}, "record": "freq"}
  }');
