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
