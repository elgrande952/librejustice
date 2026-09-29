DROP TABLE resolution_run_diff;
DROP TABLE resolution_run;
DROP TABLE citation_occurrence_link;
DROP TABLE citation_gold_anchor;
DROP TABLE legal_citation;
DROP TABLE citation_key;

CREATE TABLE legal_citation (
    decision_id     int8 NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    char_start      int4 NOT NULL,
    char_end        int4 NOT NULL,
    ref_text_uid    text REFERENCES legal_text(text_uid),
    ref_num_key     text,
    extract_version int2 NOT NULL,
    PRIMARY KEY (decision_id, char_start),
    CHECK (char_end > char_start),
    CHECK (ref_num_key IS NULL OR ref_text_uid IS NOT NULL)
);
CREATE INDEX idx_lc_ref ON legal_citation (ref_text_uid, ref_num_key)
    WHERE ref_text_uid IS NOT NULL;
ALTER TABLE legal_citation SET (fillfactor = 100,
    autovacuum_vacuum_scale_factor = 0.02, autovacuum_analyze_scale_factor = 0.01);
