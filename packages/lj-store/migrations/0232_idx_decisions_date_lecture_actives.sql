-- lj:sans-transaction
CREATE INDEX CONCURRENTLY idx_decisions_date_lecture_actives
ON decisions (date_lecture DESC NULLS LAST, id)
WHERE deleted_at IS NULL;
