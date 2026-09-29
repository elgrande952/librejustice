ALTER INDEX idx_chunks_legal_article_composite SET (fastupdate = off);
SELECT gin_clean_pending_list('idx_chunks_legal_article_composite');

ALTER INDEX idx_chunks_legal_instruments SET (fastupdate = off);
SELECT gin_clean_pending_list('idx_chunks_legal_instruments');
