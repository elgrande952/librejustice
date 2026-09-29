CREATE INDEX idx_decisions_jur_code_date
    ON decisions (jurisdiction_code, date_lecture DESC);
