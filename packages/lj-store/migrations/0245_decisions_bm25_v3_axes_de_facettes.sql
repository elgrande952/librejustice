-- lj:sans-transaction
-- Migration 0245. decisions_bm25 reconstruit avec les six axes de facettes qui
-- lui manquaient, plus deleted_at.
--
-- Les facettes et le total exacts se comptent désormais dans l'index, en une
-- passe paradedb.aggregate, pour la recherche textuelle comme pour le parcours.
-- Six colonnes entrent en fast field : jurisdiction_code, chamber_key,
-- matiere_uid, legal_domain_uid, solution_uid, office_uid. Mesuré le 22/08 sur le
-- corpus entier : huit axes plus le total en 346 ms sur une requête BM25 qui
-- apparie 657 577 décisions, contre 3 399 ms pour la même chose en SQL.
--
-- deleted_at entre pour que l'agrégat puisse s'en tenir au fonds vif, par
-- must_not => paradedb.exists('deleted_at') : sans lui il compte le mort avec le
-- vif. Vérifié à l'unité, 566 092 sous filtre CC pour 566 093 lignes.
--
-- Pas de champ année, et ce n'est pas un oubli. L'indexation d'expression lit le
-- mauvais datum dès que la chaîne de cast traverse un type non textuel :
-- extract(year ...)::text rend des clés de 50 000 zéros, la variante par ::int
-- écrase tous les millésimes dans un seul seau. L'axe année sort d'un
-- date_histogram en seaux d'un jour sur date_lecture, déjà en fast field, replié
-- par année côté Rust : un jour ne chevauche jamais deux années civiles, donc la
-- somme est exacte, et le balayage étant partagé l'histogramme ne coûte rien de
-- plus dans le même appel, 456 ms contre 457 sans lui.
--
-- Même procédure que la 0192/0206/0213 : construction concurrente d'un second
-- index, bascule transactionnelle par la 0246. La règle ParadeDB d'un seul index
-- bm25 par relation ne refuse que la création NON concurrente ; les deux index
-- coexistent le temps de la bascule.
--
-- Hors transaction, donc sans rollback : une interruption laisse un index
-- INVALIDE que le run suivant refusera de recréer. Reprise = DROP INDEX
-- decisions_bm25_v3, puis rejouer. Mesuré : 42 min de construction, et l'index
-- obtenu fait 14 Go contre 16 pour celui en place, le ballonnement accumulé
-- depuis la 0213 pesant plus lourd que les six colonnes ajoutées.

CREATE INDEX CONCURRENTLY decisions_bm25_v3 ON decisions
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
  ((matiere_uid)::pdb.literal),
  ((legal_domain_uid)::pdb.literal),
  ((solution_uid)::pdb.literal),
  ((office_uid)::pdb.literal),
  date_lecture,
  deleted_at
) WITH (key_field=id, text_fields='{"full_text": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"}, "search_title": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"}}');
