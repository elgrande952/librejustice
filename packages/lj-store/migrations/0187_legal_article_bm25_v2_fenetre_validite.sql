-- lj:sans-transaction
-- Migration 0187 — La fenêtre de validité et `kind` entrent dans l'index des
-- articles, construit à côté de l'index vivant.
--
-- « En vigueur » cesse d'être le littéral `status = 'VIGUEUR'` et devient
-- `date_debut <= $d AND (date_fin IS NULL OR date_fin >= $d)`. Pour que le
-- scan single-table TopK reste index-only, les deux dates
-- doivent être des fast fields.
--
-- Mesuré sur la prod avant rebuild : la même requête passe de 146 ms (status,
-- indexé) à 2 829 ms avec la fenêtre — le plan tombe en `heap_filter` sur les
-- deux dates et touche 1,06 M de buffers contre 3 k. Vérifié sur table sonde
-- avec les dates indexées : `date_debut <= $d` devient un `range` Tantivy et
-- `date_fin IS NULL OR date_fin >= $d` un `boolean.should` (`must_not exists`
-- + `range`), sans aucun `heap_filter`.
--
-- Un index bm25 est couvrant : ses colonnes sont fixées à la création. La
-- reconstruction passe par un second index en construction concurrente, que
-- ParadeDB admet là où il refuse deux index bm25 par la voie ordinaire, et
-- qu'il documente comme LA procédure de changement de schéma. La
-- recherche d'articles n'est donc pas coupée : l'ancien index sert pendant la
-- construction, la bascule est faite par la 0189 — contrairement au DROP +
-- CREATE de la migration 0165, qui coupait ~5-10 min.
--
-- Hors transaction, donc sans rollback : une interruption laisse un index
-- INVALIDE que le run suivant refusera de recréer. Reprise = DROP INDEX
-- legal_article_bm25_v2, puis rejouer.
--
-- `kind` rejoint l'index en littéral : la recherche d'articles ne
-- sert que `kind = 'article'`, égalité sur deux valeurs, poussée en `term`
-- comme `status` l'était.
--
-- `status` reste dans l'index : il ne filtre plus la recherche mais alimente
-- l'agrégat de facettes et s'affiche en badge sur la carte de résultat. Champs
-- texte inchangés (tokenizers/record identiques : la sémantique de scoring des
-- jambes titre, corps et num ne bouge pas).

CREATE INDEX CONCURRENTLY legal_article_bm25_v2 ON legal_article
USING bm25 (
    id,
    search_title,
    texte,
    num,
    (source::pdb.literal),
    (text_uid::pdb.literal),
    (status::pdb.literal),
    (jurisdiction::pdb.literal),
    (nature::pdb.literal),
    (slug::pdb.literal),
    (kind::pdb.literal),
    searchable,
    date_debut,
    date_fin
)
WITH (
  key_field = 'id',
  text_fields = '{
    "search_title": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "freq"},
    "texte":        {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "freq"},
    "num":          {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true}, "record": "basic"}
  }'
);
