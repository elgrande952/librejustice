-- lj:sans-transaction
CREATE INDEX CONCURRENTLY idx_decisions_jur_code_date_v2
ON decisions (jurisdiction_code, date_lecture DESC, id)
WHERE deleted_at IS NULL;
