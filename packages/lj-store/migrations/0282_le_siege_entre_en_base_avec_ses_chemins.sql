-- Additive: the old server still reads `chamber_key`, written until T+2, so
-- this applies with the server up. The re-extraction fills the two columns.

ALTER TABLE decisions
  ADD COLUMN seat       jsonb,
  ADD COLUMN seat_paths text[];
