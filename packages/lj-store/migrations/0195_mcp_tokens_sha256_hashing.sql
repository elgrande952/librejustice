ALTER TABLE mcp_tokens ADD COLUMN access_token_sha256 BYTEA;
UPDATE mcp_tokens SET access_token_sha256 = sha256(access_token::bytea);
ALTER TABLE mcp_tokens ALTER COLUMN access_token_sha256 SET NOT NULL;

ALTER TABLE mcp_tokens DROP CONSTRAINT mcp_tokens_pkey;
ALTER TABLE mcp_tokens ADD PRIMARY KEY (access_token_sha256);
ALTER TABLE mcp_tokens ALTER COLUMN access_token DROP NOT NULL;

ALTER TABLE mcp_refresh_tokens ADD COLUMN refresh_token_sha256 BYTEA;
UPDATE mcp_refresh_tokens SET refresh_token_sha256 = sha256(refresh_token::bytea);
ALTER TABLE mcp_refresh_tokens ALTER COLUMN refresh_token_sha256 SET NOT NULL;

ALTER TABLE mcp_refresh_tokens DROP CONSTRAINT mcp_refresh_tokens_pkey;
ALTER TABLE mcp_refresh_tokens ADD PRIMARY KEY (refresh_token_sha256);
ALTER TABLE mcp_refresh_tokens ALTER COLUMN refresh_token DROP NOT NULL;

CREATE FUNCTION mcp_token_derive_sha256() RETURNS trigger AS $$
BEGIN
    IF TG_TABLE_NAME = 'mcp_tokens' THEN
        NEW.access_token_sha256 := COALESCE(
            NEW.access_token_sha256, sha256(NEW.access_token::bytea)
        );
    ELSE
        NEW.refresh_token_sha256 := COALESCE(
            NEW.refresh_token_sha256, sha256(NEW.refresh_token::bytea)
        );
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER mcp_tokens_sha256
    BEFORE INSERT ON mcp_tokens
    FOR EACH ROW EXECUTE FUNCTION mcp_token_derive_sha256();

CREATE TRIGGER mcp_refresh_tokens_sha256
    BEFORE INSERT ON mcp_refresh_tokens
    FOR EACH ROW EXECUTE FUNCTION mcp_token_derive_sha256();
