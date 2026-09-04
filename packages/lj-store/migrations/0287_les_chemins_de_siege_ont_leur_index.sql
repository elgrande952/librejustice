-- lj:sans-transaction
-- Migration 0287. The seat paths get their GIN index.
--
-- The browse leg filters the path in plain SQL, outside the BM25 index, so an
-- overlap on 3.7M rows needs its own access path. `fastupdate=off` as on the
-- other array GIN of the table: the pending list makes the writer pay a merge
-- at an unpredictable moment.

CREATE INDEX CONCURRENTLY idx_decisions_seat_paths ON decisions
USING gin (seat_paths) WITH (fastupdate=off) WHERE (deleted_at IS NULL);
