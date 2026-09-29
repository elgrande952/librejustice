ALTER TABLE decisions
    ADD COLUMN IF NOT EXISTS canonical_ref_ambiguous boolean NOT NULL DEFAULT false;

DROP TABLE IF EXISTS ambiguous_canonical_ref;
