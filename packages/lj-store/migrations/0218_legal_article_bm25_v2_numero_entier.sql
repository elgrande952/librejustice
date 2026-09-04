-- lj:sans-transaction
-- Migration 0218 — legal_article_bm25 reconstruit avec le tokenizer qui garde un
-- numéro entier.
--
-- Le champ `num` porte 45 274 valeurs à séparateur (« 2.1 » × 804, « 3.1 » × 806) :
-- l'ancien pattern les coupe, donc la forme pointée cherchée en conjonction
-- (`match('num', '2.1', conjunction_mode)`) rend 8 188 articles — tout ce qui
-- porte un `2` ET un `1`. Avec le numéro entier, 865, dix fois plus précis.
--
-- Le pattern est celui de `lj_core::body_tok::TOKEN_PATTERN`, recopié ici parce
-- que le SQL ne peut pas l'importer. Champs repris à l'identique de l'index en
-- place, seul le pattern change.
--
-- Hors transaction, donc sans rollback : une interruption laisse un index
-- INVALIDE que le run suivant refusera de recréer. Reprise = DROP INDEX
-- legal_article_bm25_v2, puis rejouer. ~15 min pour 3,6 M d'articles.

CREATE INDEX CONCURRENTLY legal_article_bm25_v2 ON legal_article
USING bm25 (
  id,
  search_title,
  texte,
  num,
  ((source)::pdb.literal),
  ((text_uid)::pdb.literal),
  ((status)::pdb.literal),
  ((jurisdiction)::pdb.literal),
  ((nature)::pdb.literal),
  ((slug)::pdb.literal),
  ((kind)::pdb.literal),
  searchable,
  date_debut,
  date_fin
) WITH (key_field=id, text_fields='{
    "search_title": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "freq"},
    "texte":        {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "freq"},
    "num":          {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true}, "record": "basic"}
  }');
