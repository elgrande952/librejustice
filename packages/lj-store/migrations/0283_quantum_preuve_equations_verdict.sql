ALTER TABLE quantum_observation
  ADD COLUMN preuve    text,
  ADD COLUMN equations smallint;

ALTER TABLE decision_quantum
  ADD COLUMN verdict jsonb;
