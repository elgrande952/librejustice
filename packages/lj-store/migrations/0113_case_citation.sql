CREATE TABLE case_citation (
    decision_id        int8 NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    char_start         int4 NOT NULL,
    char_end           int4 NOT NULL,
    target_ref         text NOT NULL,
    target_decision_id int8 REFERENCES decisions(id) ON DELETE SET NULL,
    extract_version    int2 NOT NULL,
    PRIMARY KEY (decision_id, char_start),
    CHECK (char_end > char_start)
);

CREATE INDEX case_citation_pending_target_idx
    ON case_citation (target_ref) WHERE target_decision_id IS NULL;

CREATE INDEX case_citation_target_decision_idx
    ON case_citation (target_decision_id) WHERE target_decision_id IS NOT NULL;
ALTER TABLE case_citation SET (fillfactor = 100,
    autovacuum_vacuum_scale_factor = 0.02, autovacuum_analyze_scale_factor = 0.01);

CREATE INDEX idx_decisions_docket_numbers
    ON decisions USING GIN (docket_numbers) WHERE deleted_at IS NULL;
