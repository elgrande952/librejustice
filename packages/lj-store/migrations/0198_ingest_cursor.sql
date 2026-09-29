CREATE TABLE IF NOT EXISTS ingest_cursor (
    job        text PRIMARY KEY,
    position   text NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);
