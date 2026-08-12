-- Migration 0195 : empreinte SHA-256 des jetons MCP — étape « expand ».
--
-- AUCUN code ne lit encore `*_sha256` : c'est ce qui rend cette migration sûre.
-- Le deploy précède la migration, donc le code neuf tourne un moment
-- contre le schéma ancien ; en n'introduisant ici que de l'additif invisible, cette
-- fenêtre ne peut rien casser. Le passage de la vérification sur l'empreinte, et le
-- retrait de la colonne en clair, partent en release 2.
--
-- `sha256()` est un built-in Postgres (PG 11+), pas besoin de `pgcrypto`.

-- ── mcp_tokens ──────────────────────────────────────────────────────────────
ALTER TABLE mcp_tokens ADD COLUMN access_token_sha256 BYTEA;
UPDATE mcp_tokens SET access_token_sha256 = sha256(access_token::bytea);
ALTER TABLE mcp_tokens ALTER COLUMN access_token_sha256 SET NOT NULL;

-- La PK portait la valeur en clair. On la déplace sur l'empreinte, qui est tout
-- aussi unique (32 octets de SHA-256 sur 32 octets d'entropie OS), et on libère la
-- colonne en clair pour que la release 2 puisse cesser de l'écrire sans la voir
-- refuser par un NOT NULL.
ALTER TABLE mcp_tokens DROP CONSTRAINT mcp_tokens_pkey;
ALTER TABLE mcp_tokens ADD PRIMARY KEY (access_token_sha256);
ALTER TABLE mcp_tokens ALTER COLUMN access_token DROP NOT NULL;

-- ── mcp_refresh_tokens ──────────────────────────────────────────────────────
ALTER TABLE mcp_refresh_tokens ADD COLUMN refresh_token_sha256 BYTEA;
UPDATE mcp_refresh_tokens SET refresh_token_sha256 = sha256(refresh_token::bytea);
ALTER TABLE mcp_refresh_tokens ALTER COLUMN refresh_token_sha256 SET NOT NULL;

ALTER TABLE mcp_refresh_tokens DROP CONSTRAINT mcp_refresh_tokens_pkey;
ALTER TABLE mcp_refresh_tokens ADD PRIMARY KEY (refresh_token_sha256);
ALTER TABLE mcp_refresh_tokens ALTER COLUMN refresh_token DROP NOT NULL;

-- ── Dérivation automatique pendant la transition ────────────────────────────
--
-- Entre cette migration et la release 2, l'ANCIEN code tourne toujours et émet des
-- jetons en n'écrivant que le clair. Sans ce trigger ces lignes naîtraient sans
-- empreinte — donc en violation du NOT NULL ci-dessus, et leurs porteurs seraient
-- éjectés au switch. Il se supprime en release 2, quand le code écrit l'empreinte
-- lui-même.
--
-- `COALESCE` et pas un écrasement : le code de la release 2 fournit l'empreinte et
-- n'écrit plus le clair, il ne doit pas se faire recalculer un NULL par-dessus.
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
