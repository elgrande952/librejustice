ALTER INDEX idx_decisions_docket_keys SET (fastupdate = off);

SELECT gin_clean_pending_list('idx_decisions_docket_keys');
