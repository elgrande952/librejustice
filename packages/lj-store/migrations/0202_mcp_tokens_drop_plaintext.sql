DROP TRIGGER mcp_tokens_sha256 ON mcp_tokens;
DROP TRIGGER mcp_refresh_tokens_sha256 ON mcp_refresh_tokens;
DROP FUNCTION mcp_token_derive_sha256();

ALTER TABLE mcp_tokens DROP COLUMN access_token;
ALTER TABLE mcp_refresh_tokens DROP COLUMN refresh_token;
