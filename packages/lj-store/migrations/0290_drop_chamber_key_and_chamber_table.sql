DROP INDEX idx_decisions_chamber_key;

ALTER TABLE decisions DROP COLUMN chamber_key;

DROP TABLE chamber;
