ALTER TABLE assertion
    ADD CONSTRAINT assertion_text_fk FOREIGN KEY (text_uid)
    REFERENCES legal_text(text_uid) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE assertion_compile_state
    ADD CONSTRAINT assertion_compile_state_text_fk FOREIGN KEY (text_uid)
    REFERENCES legal_text(text_uid) ON UPDATE CASCADE ON DELETE CASCADE;

CREATE INDEX assertion_version_idx ON assertion (text_uid, num_key, version_start);

ALTER TABLE expression DROP COLUMN source;
DROP INDEX assertion_compile_state_journal_idx;

UPDATE facet_value SET label = 'France' WHERE uid = 'geo:FR';
