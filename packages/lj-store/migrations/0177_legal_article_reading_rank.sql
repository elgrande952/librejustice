-- Ordre de lecture persisté.
--
-- Rang de lecture d'un article dans son texte, calculé par lj_core::article_order
-- et écrit par le seul finisseur `refresh_reading_ranks`. Propriété de
-- (text_uid, num_key) : toutes les versions datées d'un article partagent le
-- rang. NULL = à (re)calculer — c'est l'unique marqueur de saleté, et au sortir
-- de cette migration il vaut pour tout le corpus : la première passe du
-- finisseur EST le backfill.
ALTER TABLE legal_article ADD COLUMN reading_rank integer;

-- Lecture : sommaire, vue à plat et contexte ordonnent par (text_uid, rang).
CREATE INDEX IF NOT EXISTS idx_legal_article_reading_rank
  ON legal_article (text_uid, reading_rank);

-- Détection des textes sales, sans scanner les 2,24 M lignes.
CREATE INDEX IF NOT EXISTS idx_legal_article_reading_rank_stale
  ON legal_article (text_uid) WHERE reading_rank IS NULL;
