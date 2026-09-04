-- Migration 0248 : la donnée de compte passe dans le schéma `comptes`.
--
-- Le `search_path` posé ici suffit à une base neuve, pas à une base vivante :
-- une connexion tient le chemin reçu à son ouverture, donc la prod joue cet
-- ALTER DATABASE à la main puis redémarre `app` et `cron` AVANT d'appliquer la
-- migration. Sans ce temps, le pool ne résout plus `users` après le COMMIT.

DO $$
BEGIN
    EXECUTE format(
        'ALTER DATABASE %I SET search_path = public, paradedb, comptes',
        current_database()
    );
END
$$;

CREATE SCHEMA comptes;

ALTER TABLE users               SET SCHEMA comptes;
ALTER TABLE user_identities     SET SCHEMA comptes;
ALTER TABLE user_bookmarks      SET SCHEMA comptes;
ALTER TABLE user_search_history SET SCHEMA comptes;
ALTER TABLE user_decision_views SET SCHEMA comptes;
ALTER TABLE web_sessions        SET SCHEMA comptes;
ALTER TABLE auth_email_tokens   SET SCHEMA comptes;
ALTER TABLE mcp_clients         SET SCHEMA comptes;
ALTER TABLE mcp_tokens          SET SCHEMA comptes;
ALTER TABLE mcp_refresh_tokens  SET SCHEMA comptes;
ALTER TABLE mcp_auth_codes      SET SCHEMA comptes;
