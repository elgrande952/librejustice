ALTER TABLE users ADD COLUMN sessions_valid_from TIMESTAMPTZ;

COMMENT ON COLUMN users.sessions_valid_from IS
    'Tout JWT Supabase dont iat est antérieur est refusé. NULL = jamais révoqué. '
    'Posé par la déconnexion globale ; il n''existe pas de table de sessions.';
