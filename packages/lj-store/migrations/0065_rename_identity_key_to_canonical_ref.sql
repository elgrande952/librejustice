ALTER TABLE decisions
    RENAME COLUMN identity_key TO canonical_ref;

ALTER INDEX idx_decisions_identity_key
    RENAME TO idx_decisions_canonical_ref;
