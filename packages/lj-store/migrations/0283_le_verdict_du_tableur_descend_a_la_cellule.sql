-- Additive, applies with the server up: nothing reads the three columns yet.
--
-- `preuve` says what determines the amount of an observation, by the
-- spreadsheet: two agreeing paths, the reading alone, or a single relation.
-- `equations` counts the relations that hold its cell. `verdict` keeps the
-- spreadsheet's whole verdict, of which three columns next to it were the only
-- stored part.

ALTER TABLE quantum_observation
  ADD COLUMN preuve    text,
  ADD COLUMN equations smallint;

ALTER TABLE decision_quantum
  ADD COLUMN verdict jsonb;
