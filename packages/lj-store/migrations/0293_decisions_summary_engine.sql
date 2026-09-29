ALTER TABLE decisions ADD COLUMN summary_engine SMALLINT;

COMMENT ON COLUMN decisions.summary_engine IS
  'The (prompt, model) pair that wrote summary. Codes in lj_core::summary::SummaryEngine. NULL predates the column.';
