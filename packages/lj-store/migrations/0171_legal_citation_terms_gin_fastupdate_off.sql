ALTER INDEX legal_citation_terms_idx SET (fastupdate = off);
SELECT gin_clean_pending_list('legal_citation_terms_idx');
