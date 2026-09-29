ALTER INDEX entity_folded_all_idx SET (fastupdate = off);
SELECT gin_clean_pending_list('entity_folded_all_idx');
