ALTER TABLE legal_article ADD COLUMN reading_rank integer;

CREATE INDEX IF NOT EXISTS idx_legal_article_reading_rank
  ON legal_article (text_uid, reading_rank);

CREATE INDEX IF NOT EXISTS idx_legal_article_reading_rank_stale
  ON legal_article (text_uid) WHERE reading_rank IS NULL;
