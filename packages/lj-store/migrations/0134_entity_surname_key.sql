ALTER TABLE entity ADD COLUMN surname_key text;
CREATE INDEX entity_surname_key_idx ON entity (surname_key)
    WHERE surname_key IS NOT NULL;
