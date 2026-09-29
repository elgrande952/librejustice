SET LOCAL lock_timeout = '5s';

ALTER TABLE decisions VALIDATE CONSTRAINT decisions_public_id_present;

ALTER TABLE decisions ALTER COLUMN public_id SET NOT NULL;

ALTER TABLE decisions DROP CONSTRAINT decisions_public_id_present;
