ALTER TABLE decision_links
    ADD COLUMN target_court  text,
    ADD COLUMN target_docket text,
    ADD COLUMN target_date   date;

ALTER TABLE case_citation
    ADD COLUMN target_court  text,
    ADD COLUMN target_docket text,
    ADD COLUMN target_date   date;

ALTER TABLE text_case_citation
    ADD COLUMN target_court  text,
    ADD COLUMN target_docket text,
    ADD COLUMN target_date   date;

ALTER TABLE decision_renvoi
    ADD COLUMN target_court  text,
    ADD COLUMN target_docket text;

ALTER TABLE decision_links ALTER COLUMN target_ref DROP NOT NULL;
ALTER TABLE case_citation ALTER COLUMN target_ref DROP NOT NULL;
ALTER TABLE text_case_citation ALTER COLUMN target_ref DROP NOT NULL;

ALTER TABLE decision_renvoi DROP CONSTRAINT decision_renvoi_pkey;
ALTER TABLE decision_renvoi ALTER COLUMN target_ref DROP NOT NULL;

COMMENT ON COLUMN decision_links.target_court IS
    'jurisdiction.code of the target, NULL when the gazetteer does not hold the name the source wrote';
COMMENT ON COLUMN decision_links.target_docket IS
    'docket as written; folding to a key belongs to the join, through lj_docket_keys';

CREATE INDEX decision_links_pending_addr_idx
    ON decision_links (target_court, target_date)
    WHERE target_decision_id IS NULL;
CREATE INDEX case_citation_pending_addr_idx
    ON case_citation (target_court, target_date)
    WHERE target_decision_id IS NULL;
CREATE INDEX text_case_citation_pending_addr_idx
    ON text_case_citation (target_court, target_date)
    WHERE target_decision_id IS NULL;
CREATE INDEX decision_renvoi_pending_addr_idx
    ON decision_renvoi (target_court, target_date)
    WHERE target_decision_id IS NULL;

CREATE INDEX decision_links_pending_docket_idx
    ON decision_links USING gin (lj_docket_keys(ARRAY[target_docket]))
    WHERE target_decision_id IS NULL AND target_docket IS NOT NULL;
CREATE INDEX case_citation_pending_docket_idx
    ON case_citation USING gin (lj_docket_keys(ARRAY[target_docket]))
    WHERE target_decision_id IS NULL AND target_docket IS NOT NULL;
CREATE INDEX text_case_citation_pending_docket_idx
    ON text_case_citation USING gin (lj_docket_keys(ARRAY[target_docket]))
    WHERE target_decision_id IS NULL AND target_docket IS NOT NULL;
