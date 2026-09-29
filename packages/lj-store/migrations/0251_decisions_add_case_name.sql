ALTER TABLE decisions
    ADD COLUMN case_name text,
    ADD COLUMN case_name_keys text[];

CREATE INDEX decisions_case_name_keys_idx ON decisions USING gin (case_name_keys)
    WHERE case_name_keys IS NOT NULL;
