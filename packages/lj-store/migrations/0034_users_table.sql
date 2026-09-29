CREATE TABLE users (
    sub           TEXT PRIMARY KEY,
    email         TEXT,
    display_name  TEXT,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_users_last_seen_at ON users (last_seen_at);
