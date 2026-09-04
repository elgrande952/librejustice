-- lj:sans-transaction
-- Migration 0220 — legal_article_usage_bm25 reconstruit avec le tokenizer qui
-- garde un numéro entier.
--
-- Le champ `terms` agrège les formes citées d'un article : il porte les mêmes
-- numéros à séparateur que `legal_article.num`, et la jambe `cc-usage` est celle
-- qui pèse le plus dans le banc (nDCG@10 0,42 → 0,60). Il garde son `_`, que les
-- clés d'usage utilisent — c'est la seule variation du pattern dans le dépôt.
--
-- Le pattern est celui de `lj_core::body_tok::TOKEN_PATTERN` plus ce `_`, recopié
-- ici parce que le SQL ne peut pas l'importer. Champs repris à l'identique de
-- l'index en place, seul le pattern change.
--
-- Hors transaction, donc sans rollback : une interruption laisse un index
-- INVALIDE que le run suivant refusera de recréer. Reprise = DROP INDEX
-- legal_article_usage_bm25_v2, puis rejouer. ~20 s pour 56 587 lignes.

CREATE INDEX CONCURRENTLY legal_article_usage_bm25_v2 ON legal_article_usage
USING bm25 (
  id,
  terms,
  ((text_uid)::pdb.literal),
  ((num_key)::pdb.literal)
) WITH (key_field=id, text_fields='{
    "terms": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}_-]+", "ascii_folding": true}, "record": "freq"}
  }');
