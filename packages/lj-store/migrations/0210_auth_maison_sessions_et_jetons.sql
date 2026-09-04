-- Migration 0210 — L'identité passe dans notre base : secrets de connexion sur
-- `users`, sessions web et jetons de mail à usage unique.

-- Les UUID ne changent pas : `users.sub` reste la PK, référencée par six FK.
-- `password_hash` accueille le bcrypt `$2a$10$` importé de Supabase tel quel ;
-- il reste NULL pour les comptes qui n'entrent que par Google.
ALTER TABLE users
    ADD COLUMN password_hash      text,
    ADD COLUMN email_confirmed_at timestamptz,
    ADD COLUMN google_sub         text;

-- L'email est la clé de rapprochement à la connexion, sur 95 comptes pour 95
-- adresses distinctes ; l'unicité insensible à la casse la rend fiable.
CREATE UNIQUE INDEX users_email_lower_uidx ON users (lower(email)) WHERE email IS NOT NULL;
CREATE UNIQUE INDEX users_google_sub_uidx ON users (google_sub) WHERE google_sub IS NOT NULL;

-- Session web : seule l'empreinte SHA-256 du cookie est stockée, comme
-- `mcp_tokens`. Un vol de la base ne donne aucun cookie utilisable.
-- `expires_at` glisse à l'usage, sans dépasser `absolute_expires_at`.
CREATE TABLE web_sessions (
    session_sha256      bytea PRIMARY KEY,
    user_sub            text NOT NULL REFERENCES users(sub) ON DELETE CASCADE,
    expires_at          timestamptz NOT NULL,
    absolute_expires_at timestamptz NOT NULL,
    created_at          timestamptz NOT NULL DEFAULT now(),
    last_used_at        timestamptz NOT NULL DEFAULT now(),
    CHECK (absolute_expires_at >= expires_at)
);
CREATE INDEX web_sessions_expires_at_idx ON web_sessions (expires_at);
CREATE INDEX web_sessions_user_sub_idx ON web_sessions (user_sub);

-- Jetons de mail : confirmation d'inscription et réinitialisation. Usage unique
-- (`used_at`), courte vie, empreinte seule — le lien envoyé est le seul endroit
-- où le secret existe en clair.
CREATE TABLE auth_email_tokens (
    token_sha256 bytea PRIMARY KEY,
    user_sub     text NOT NULL REFERENCES users(sub) ON DELETE CASCADE,
    purpose      text NOT NULL CHECK (purpose IN ('confirmation', 'recovery')),
    expires_at   timestamptz NOT NULL,
    created_at   timestamptz NOT NULL DEFAULT now(),
    used_at      timestamptz
);
CREATE INDEX auth_email_tokens_expires_at_idx ON auth_email_tokens (expires_at);
CREATE INDEX auth_email_tokens_user_purpose_idx ON auth_email_tokens (user_sub, purpose);
