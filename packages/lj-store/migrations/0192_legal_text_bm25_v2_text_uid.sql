-- lj:sans-transaction
-- Migration 0192 — text_uid entre dans legal_text_body_bm25.
--
-- L'agrégat de facettes du membre « textes à corps » pousse
-- paradedb.term('text_uid', …) quand l'axe code est actif, et le champ n'a
-- jamais été dans l'index → ERROR XX000 sur /api/search-textes?…&code=….
-- Même procédure que la 0185 : construction concurrente d'un
-- second index bm25, bascule transactionnelle par la 0193. Tous les champs
-- de la 0185 repris à l'identique, text_uid s'ajoute en littéral.
--
-- Hors transaction, donc sans rollback : une interruption laisse un index
-- INVALIDE que le run suivant refusera de recréer. Reprise = DROP INDEX
-- legal_text_body_bm25_v2, puis rejouer.

CREATE INDEX CONCURRENTLY legal_text_body_bm25_v2 ON legal_text
USING bm25 (
    id,
    title,
    body,
    (text_uid::pdb.literal),
    (nature::pdb.literal),
    (jurisdiction::pdb.literal),
    (slug::pdb.literal),
    (role::pdb.literal),
    ((coalesce(status, 'VIGUEUR'))::pdb.literal('alias=status_eff')),
    ((upper(nature))::pdb.literal('alias=nature_up')),
    ((lower(nature))::pdb.literal('alias=source')),
    ((body IS NOT NULL)::pdb.alias('has_body'))
)
WITH (
  key_field = 'id',
  text_fields = '{
    "title": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"},
    "body":  {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"}
  }'
);
