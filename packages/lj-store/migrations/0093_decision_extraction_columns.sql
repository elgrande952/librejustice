ALTER TABLE decisions ADD COLUMN applicant_companies      TEXT[];
ALTER TABLE decisions ADD COLUMN applicant_individuals    TEXT[];
ALTER TABLE decisions ADD COLUMN applicant_counsel_names  TEXT[];
ALTER TABLE decisions ADD COLUMN applicant_law_firms      TEXT[];
ALTER TABLE decisions ADD COLUMN defendant_companies      TEXT[];
ALTER TABLE decisions ADD COLUMN defendant_individuals    TEXT[];
ALTER TABLE decisions ADD COLUMN defendant_counsel_names  TEXT[];
ALTER TABLE decisions ADD COLUMN defendant_law_firms      TEXT[];
ALTER TABLE decisions ADD COLUMN defendant_administration TEXT;
ALTER TABLE decisions ADD COLUMN intervenors             TEXT[];
ALTER TABLE decisions ADD COLUMN challenged_acts         TEXT[];

ALTER TABLE decisions ADD COLUMN decision_type              TEXT;
ALTER TABLE decisions ADD COLUMN detailed_main_outcome      TEXT;
ALTER TABLE decisions ADD COLUMN legal_domain               TEXT;
ALTER TABLE decisions ADD COLUMN jurisdiction_location_code  TEXT;
ALTER TABLE decisions ADD COLUMN jurisdiction_location_label TEXT;
ALTER TABLE decisions ADD COLUMN publication_scope          TEXT;
ALTER TABLE decisions ADD COLUMN publication_status         TEXT;
ALTER TABLE decisions ADD COLUMN search_keywords            TEXT[];
ALTER TABLE decisions ADD COLUMN lower_decisions            JSONB;

DROP TABLE IF EXISTS gold_annotation;
