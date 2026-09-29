DELETE FROM mcp_tokens     t WHERE NOT EXISTS (SELECT 1 FROM mcp_clients k WHERE k.client_id = t.client_id);
DELETE FROM mcp_auth_codes c WHERE NOT EXISTS (SELECT 1 FROM mcp_clients k WHERE k.client_id = c.client_id);

ALTER TABLE mcp_tokens
    ADD CONSTRAINT mcp_tokens_client_fk
    FOREIGN KEY (client_id) REFERENCES mcp_clients (client_id) ON DELETE CASCADE;

ALTER TABLE mcp_auth_codes
    ADD CONSTRAINT mcp_auth_codes_client_fk
    FOREIGN KEY (client_id) REFERENCES mcp_clients (client_id) ON DELETE CASCADE;
