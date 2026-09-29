ALTER TABLE decisions
    ADD COLUMN themes text[] NOT NULL DEFAULT '{}';
