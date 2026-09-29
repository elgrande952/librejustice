ALTER TABLE text_place_coverage ADD COLUMN journal_code text;
ALTER TABLE text_place_coverage ADD COLUMN nature text;

CREATE INDEX text_place_coverage_corpus_idx
    ON text_place_coverage (place, journal_code, nature, starts_on, ends_on);
DROP INDEX text_place_coverage_place_idx;
