CREATE TABLE api_keys (
    key_sha256   bytea PRIMARY KEY,
    user_sub     text NOT NULL REFERENCES users(sub) ON DELETE CASCADE,
    label        text NOT NULL,
    prefix       text NOT NULL,
    created_at   timestamptz NOT NULL DEFAULT now(),
    last_used_at timestamptz,
    revoked_at   timestamptz
);

CREATE INDEX api_keys_user_sub_idx ON api_keys (user_sub) WHERE revoked_at IS NULL;
