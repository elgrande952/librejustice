DROP INDEX IF EXISTS decisions_title_bm25;

CREATE INDEX decisions_title_bm25 ON decisions
USING bm25 (id, search_title)
WITH (
  key_field = 'id',
  text_fields = '{"search_title": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true}, "stopwords_language": "French", "record": "position"}}'
);
