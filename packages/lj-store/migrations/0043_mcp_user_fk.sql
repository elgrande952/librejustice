DELETE FROM mcp_tokens     t WHERE NOT EXISTS (SELECT 1 FROM users u WHERE u.sub = t.user_id);
DELETE FROM mcp_auth_codes c WHERE NOT EXISTS (SELECT 1 FROM users u WHERE u.sub = c.user_id);

ALTER TABLE mcp_tokens
    ADD CONSTRAINT mcp_tokens_user_fk
    FOREIGN KEY (user_id) REFERENCES users (sub) ON DELETE CASCADE;

ALTER TABLE mcp_auth_codes
    ADD CONSTRAINT mcp_auth_codes_user_fk
    FOREIGN KEY (user_id) REFERENCES users (sub) ON DELETE CASCADE;
