SET LOCAL lock_timeout = '5s';

ALTER TABLE decisions
  ADD CONSTRAINT decisions_public_id_present CHECK (public_id IS NOT NULL) NOT VALID;
