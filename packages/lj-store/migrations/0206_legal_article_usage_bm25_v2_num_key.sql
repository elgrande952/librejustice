-- lj:sans-transaction
-- Migration 0206 — num_key entre dans legal_article_usage_bm25.
--
-- Jambe « numéro cité » de la recherche d'articles : la clause
-- usage scopée filtre `num_key = ANY($numéros cités)` dans le scan ParadeDB.
-- Sans le champ dans l'index, ParadeDB replie le filtre en `heap_filter` et
-- lit toute la sélection des grammes avant de couper — mesuré 60 à 180 ms par
-- recherche, contre 22 ms quand le filtre est poussé. Exactement le geste que
-- la 0169 a fait pour `text_uid` (jambe « code nommé »).
--
-- Même procédure que la 0192 : construction concurrente d'un second
-- index bm25, bascule transactionnelle par la 0207. Champs de la 0169 repris à
-- l'identique, num_key s'ajoute en littéral.
--
-- Hors transaction, donc sans rollback : une interruption laisse un index
-- INVALIDE que le run suivant refusera de recréer. Reprise = DROP INDEX
-- legal_article_usage_bm25_v2, puis rejouer.

CREATE INDEX CONCURRENTLY legal_article_usage_bm25_v2 ON legal_article_usage
USING bm25 (
  id,
  terms,
  ((text_uid)::pdb.literal),
  ((num_key)::pdb.literal)
) WITH (key_field=id, text_fields='{
    "terms": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}_-]+", "ascii_folding": true}, "record": "freq"}
  }');
