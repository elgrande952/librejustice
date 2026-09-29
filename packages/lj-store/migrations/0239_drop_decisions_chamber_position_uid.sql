ALTER TABLE decisions
  DROP COLUMN chamber_position,
  DROP COLUMN chamber_uid;

DELETE FROM facet_value WHERE facet = 'chamber';
