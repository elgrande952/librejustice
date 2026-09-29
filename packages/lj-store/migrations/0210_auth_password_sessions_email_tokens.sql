ALTER TABLE users
    ADD COLUMN password_hash      text,
    ADD COLUMN email_confirmed_at timestamptz,
    ADD COLUMN google_sub         text;

CREATE UNIQUE INDEX users_email_lower_uidx ON users (lower(email)) WHERE email IS NOT NULL;
CREATE UNIQUE INDEX users_google_sub_uidx ON users (google_sub) WHERE google_sub IS NOT NULL;

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
