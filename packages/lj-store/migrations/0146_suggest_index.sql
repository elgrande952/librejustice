CREATE TABLE suggest_index (
    key      text        PRIMARY KEY,
    built_at timestamptz NOT NULL DEFAULT now(),
    fst      bytea       NOT NULL
);
