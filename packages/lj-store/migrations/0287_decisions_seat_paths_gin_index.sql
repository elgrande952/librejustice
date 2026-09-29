-- lj:sans-transaction
CREATE INDEX CONCURRENTLY idx_decisions_seat_paths ON decisions
USING gin (seat_paths) WITH (fastupdate=off) WHERE (deleted_at IS NULL);
