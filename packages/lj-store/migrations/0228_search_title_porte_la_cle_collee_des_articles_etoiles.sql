-- Migration 0228 : `search_title` porte la clé collée des articles étoilés.
--
-- Le tokenizer n'admet pas `*` dans sa classe de jeton : `num = 'R*600-1'`
-- produit `r` et `600-1`, jamais `r600-1`, et la recherche de `r600-1` rendait
-- 0 ligne là où l'article est en vigueur. Aucun filtre de jeton ParadeDB ne
-- retire un caractère à l'intérieur d'un jeton, et élargir la classe à `*`
-- donnerait `r*600-1`, qui ne s'apparie pas davantage : la correction est à
-- l'écriture.
--
-- L'ajout est conditionné à l'étoile dans `num` : 21 104 versions sur 3 M la
-- portent, et l'appliquer partout doublerait le jeton de numéro sur toutes les
-- autres lignes, donc déplacerait le score BM25 de tout le référentiel.
--
-- Une colonne générée ne s'altère pas en place : DROP puis ADD réécrit la table
-- sous ACCESS EXCLUSIVE. Index BM25 déposé avant, rebâti une fois après (0079).
--
-- S'applique APRÈS la réécriture des clés en base : avant, `num_key` porte
-- encore l'étoile et le jeton ajouté double celui de `num`.

DROP INDEX IF EXISTS legal_article_bm25;

ALTER TABLE legal_article DROP COLUMN IF EXISTS search_title;

ALTER TABLE legal_article ADD COLUMN search_title TEXT GENERATED ALWAYS AS (
  COALESCE(code_title, '')
  || ', ' || num
  || CASE WHEN num LIKE '%*%' THEN ', ' || num_key ELSE '' END
  || CASE WHEN title_path IS NOT NULL AND title_path <> ''
       THEN ', ' || regexp_replace(title_path, '^.*>\s*', '')
       ELSE '' END
) STORED;

SET LOCAL maintenance_work_mem = '2GB';
SET LOCAL max_parallel_maintenance_workers = 4;

CREATE INDEX legal_article_bm25 ON legal_article
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
    "search_title": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "freq"},
    "texte":        {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "freq"},
    "num":          {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true}, "record": "basic"}
  }'
);

ANALYZE legal_article;
