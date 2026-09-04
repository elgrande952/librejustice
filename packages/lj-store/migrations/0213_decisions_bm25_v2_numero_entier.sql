-- lj:sans-transaction
-- Migration 0213 — decisions_bm25 reconstruit avec le tokenizer qui garde un
-- numéro entier.
--
-- Le pattern gagne une alternative de tête, `\p{N}+(?:[./-]\p{N}+)+`, qui colle
-- un numéro encadré de chiffres : `2008/115`, `19-20.312`, `3.000`. Mesuré sur le
-- corpus entier (banc du 14/08) : +0,42 de précision@10 sur les instruments
-- européens rares, et NDCG@10 0,49595 → 0,49672 en prose, soit rien.
--
-- La copie de référence du pattern est `lj_core::body_tok::TOKEN_PATTERN`, que le
-- tokenizer tantivy des snippets lit aussi. Les trois doivent bouger ensemble.
-- Les index du moteur `/textes` gardent le leur : ils ne sont pas mesurés ici.
--
-- Même procédure que la 0192/0206 : construction concurrente d'un second index,
-- bascule transactionnelle par la 0214. Champs repris à l'identique de l'index en
-- place, seul le pattern change.
--
-- Hors transaction, donc sans rollback : une interruption laisse un index
-- INVALIDE que le run suivant refusera de recréer. Reprise = DROP INDEX
-- decisions_bm25_v2, puis rejouer. Compter ~20 min de construction pour 12 Go.

CREATE INDEX CONCURRENTLY decisions_bm25_v2 ON decisions
USING bm25 (
  id,
  full_text,
  search_title,
  ((jurisdiction_type)::pdb.literal),
  ((legal_instruments)::pdb.literal),
  ((legal_article_composite)::pdb.literal),
  ((publication_codes)::pdb.literal),
  date_lecture
) WITH (key_field=id, text_fields='{"full_text": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"}, "search_title": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"}}');
