-- lj:sans-transaction
-- Migration 0185 — Champs fast de facettage sur legal_text_body_bm25
-- : le membre « textes à corps » des facettes de
-- /recherche-textes passe d'un GROUPING SETS sur heap à une passe
-- paradedb.aggregate, comme le membre articles. Pour cela tout
-- son prédicat doit descendre en searchqueryinput.
--
-- Champs ajoutés, un par prédicat aujourd'hui évalué sur le heap :
--   has_body    = (body IS NOT NULL)              → 43 930 lignes sur 1,33 M
--   status_eff  = coalesce(status, 'VIGUEUR')     → status NULL sur 96 % des lignes
--   role        → filtre de visibilité
--   slug        → axe « code » + slug IS NOT NULL
--   nature_up   = upper(nature)  → axe nature et filtres de nature
--   source      = lower(nature)  → axe source (la famille tient lieu de jeton)
--
-- Un index bm25 est couvrant : ses colonnes sont fixées à la création, en
-- ajouter demande une reconstruction. CONCURRENTLY est ici le seul chemin qui
-- ne coupe pas la recherche de normes — ParadeDB refuse un second index bm25
-- sur une table par la voie ordinaire, mais l'admet en construction
-- concurrente, et en fait la procédure documentée de changement de schéma
--. L'ancien index continue de servir les requêtes pendant la
-- construction ; la bascule est faite par la 0188. Champs texte inchangés
-- (tokenizers/record identiques : la sémantique de scoring des jambes titre
-- et corps ne bouge pas).
--
-- Hors transaction, donc sans rollback : une interruption laisse un index
-- INVALIDE que le run suivant refusera de recréer. Reprise = DROP INDEX
-- legal_text_body_bm25_v2, puis rejouer.

CREATE INDEX CONCURRENTLY legal_text_body_bm25_v2 ON legal_text
USING bm25 (
    id,
    title,
    body,
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
