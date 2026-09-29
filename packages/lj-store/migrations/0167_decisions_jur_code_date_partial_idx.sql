DROP INDEX idx_decisions_jur_code_date;
CREATE INDEX idx_decisions_jur_code_date ON decisions (jurisdiction_code, date_lecture DESC, id)
  WHERE deleted_at IS NULL AND public_id IS NOT NULL;
