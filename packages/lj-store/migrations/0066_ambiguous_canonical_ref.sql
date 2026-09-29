CREATE TABLE IF NOT EXISTS ambiguous_canonical_ref (
    canonical_ref TEXT PRIMARY KEY,
    n_distinct    INT,
    reason        TEXT,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
