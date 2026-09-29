DROP TABLE IF EXISTS mcp_clients CASCADE;

CREATE TABLE mcp_clients (
    client_id     TEXT PRIMARY KEY,
    name          TEXT,
    redirect_uris TEXT[] NOT NULL DEFAULT ARRAY[]::TEXT[],
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
