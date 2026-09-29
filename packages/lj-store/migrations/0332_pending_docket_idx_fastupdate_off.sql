SELECT gin_clean_pending_list('decision_links_pending_docket_idx');
SELECT gin_clean_pending_list('case_citation_pending_docket_idx');
SELECT gin_clean_pending_list('text_case_citation_pending_docket_idx');

ALTER INDEX decision_links_pending_docket_idx SET (fastupdate = off);
ALTER INDEX case_citation_pending_docket_idx SET (fastupdate = off);
ALTER INDEX text_case_citation_pending_docket_idx SET (fastupdate = off);

SELECT gin_clean_pending_list('decision_links_pending_docket_idx');
SELECT gin_clean_pending_list('case_citation_pending_docket_idx');
SELECT gin_clean_pending_list('text_case_citation_pending_docket_idx');
