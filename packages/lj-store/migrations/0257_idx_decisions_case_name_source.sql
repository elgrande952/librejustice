CREATE INDEX decisions_case_name_source_idx ON decisions (case_name_source)
    WHERE case_name_source IS NOT NULL;
