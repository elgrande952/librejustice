CREATE INDEX IF NOT EXISTS entity_uid_pattern_idx ON entity (uid text_pattern_ops);
