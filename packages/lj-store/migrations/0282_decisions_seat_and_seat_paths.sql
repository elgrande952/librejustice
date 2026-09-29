ALTER TABLE decisions
  ADD COLUMN seat       jsonb,
  ADD COLUMN seat_paths text[];
