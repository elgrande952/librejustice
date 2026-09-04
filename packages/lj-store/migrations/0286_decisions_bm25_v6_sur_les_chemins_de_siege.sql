-- lj:sans-transaction
-- Migration 0286. decisions_bm25 rebuilt on the seat paths.
--
-- The room axis becomes a path: `seat_paths` enters as a multi-valued fast
-- field where `chamber_key` was, one build carrying both moves. The swap is the
-- 0288 and waits for the code that filters on the path; in between, a filter on
-- the old column is evaluated on the table, slower and correct.
--
-- Same procedure as the 0261/0262: concurrent build, no rollback since it runs
-- outside a transaction. Recovery = DROP INDEX decisions_bm25_v6, then replay.
-- Count 25 to 50 min for 14 Go.

CREATE INDEX CONCURRENTLY decisions_bm25_v6 ON decisions
USING bm25 (
  id,
  full_text,
  search_title,
  ((jurisdiction_type)::pdb.literal),
  ((legal_instruments)::pdb.literal),
  ((legal_article_composite)::pdb.literal),
  ((publication_codes)::pdb.literal),
  ((jurisdiction_code)::pdb.literal),
  ((seat_paths)::pdb.literal),
  ((legal_domain_uid)::pdb.literal),
  ((solution_uid)::pdb.literal),
  ((office_uid)::pdb.literal),
  ((procedure_uid)::pdb.literal),
  date_lecture,
  deleted_at
) WITH (key_field=id, text_fields='{"full_text": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"}, "search_title": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"}}');
