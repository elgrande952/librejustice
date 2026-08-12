-- Migration 0202 : retrait du clair des jetons MCP — étape « contract ».
--
-- La release 2 (code) est déployée : plus rien ne lit ni n'écrit `access_token` /
-- `refresh_token`. La vérification, l'émission et la révocation portent toutes sur
-- l'empreinte, qui est la clé primaire des deux tables depuis la 0195.
--
-- Le trigger de dérivation n'a plus d'objet : il n'existait que pour la fenêtre
-- pendant laquelle l'ancien code n'écrivait que le clair.
DROP TRIGGER mcp_tokens_sha256 ON mcp_tokens;
DROP TRIGGER mcp_refresh_tokens_sha256 ON mcp_refresh_tokens;
DROP FUNCTION mcp_token_derive_sha256();

ALTER TABLE mcp_tokens DROP COLUMN access_token;
ALTER TABLE mcp_refresh_tokens DROP COLUMN refresh_token;
