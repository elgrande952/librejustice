CREATE TABLE text_place_coverage (
    text_uid text NOT NULL REFERENCES legal_text (text_uid) ON UPDATE CASCADE ON DELETE CASCADE,
    place text NOT NULL,
    starts_on date NOT NULL,
    ends_on date,
    CONSTRAINT text_place_coverage_window CHECK (ends_on IS NULL OR ends_on > starts_on)
);

CREATE INDEX text_place_coverage_place_idx ON text_place_coverage (place, starts_on, ends_on);
CREATE INDEX text_place_coverage_text_idx ON text_place_coverage (text_uid);

ALTER TABLE assertion_compile_state ADD COLUMN nature text;
