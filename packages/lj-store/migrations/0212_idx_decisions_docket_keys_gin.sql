-- lj:sans-transaction
CREATE INDEX CONCURRENTLY idx_decisions_docket_keys ON decisions
USING gin (lj_docket_keys(docket_numbers)) WHERE deleted_at IS NULL;
