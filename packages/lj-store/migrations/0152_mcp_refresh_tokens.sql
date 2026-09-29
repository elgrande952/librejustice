CREATE TABLE mcp_refresh_tokens (
    refresh_token TEXT PRIMARY KEY,
    user_id       TEXT NOT NULL REFERENCES users (sub) ON DELETE CASCADE,
    client_id     TEXT NOT NULL REFERENCES mcp_clients (client_id) ON DELETE CASCADE,
    expires_at    TIMESTAMPTZ NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX ON mcp_refresh_tokens (expires_at);
