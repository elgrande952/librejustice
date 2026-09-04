-- lj:sans-transaction
-- Migration 0261. decisions_bm25 reconstruit sans matiere_uid.
--
-- L'axe a quitté le produit : plus aucun filtre, plus aucune facette, plus
-- aucune écriture de la colonne. Le fast field est resté inerte parce qu'un DROP
-- COLUMN emporte l'index qui le référence.
--
-- Cet index est un SOUS-ensemble du _v4, non un surensemble : aucun prédicat du
-- serveur déployé ne nomme matiere_uid.
--
-- Même procédure que la 0245/0246 et la 0249/0250 : construction concurrente,
-- bascule par la 0262, DROP COLUMN par la 0263. Hors transaction, donc sans
-- rollback ; reprise = DROP INDEX decisions_bm25_v5, puis rejouer. Compter
-- ~25 min pour 16 Go.

CREATE INDEX CONCURRENTLY decisions_bm25_v5 ON decisions
USING bm25 (
  id,
  full_text,
  search_title,
  ((jurisdiction_type)::pdb.literal),
  ((legal_instruments)::pdb.literal),
  ((legal_article_composite)::pdb.literal),
  ((publication_codes)::pdb.literal),
  ((jurisdiction_code)::pdb.literal),
  ((chamber_key)::pdb.literal),
  ((legal_domain_uid)::pdb.literal),
  ((solution_uid)::pdb.literal),
  ((office_uid)::pdb.literal),
  ((procedure_uid)::pdb.literal),
  date_lecture,
  deleted_at
) WITH (key_field=id, text_fields='{"full_text": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"}, "search_title": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"}}');
