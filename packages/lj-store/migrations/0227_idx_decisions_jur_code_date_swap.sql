DROP INDEX idx_decisions_jur_code_date;

ALTER INDEX idx_decisions_jur_code_date_v2
  RENAME TO idx_decisions_jur_code_date;
