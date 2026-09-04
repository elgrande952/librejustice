-- Only the SHA-256 fingerprint is stored, as in `web_sessions`: a copy of the
-- database yields no usable key, and the secret exists in clear once, in the
-- response that creates it.
CREATE TABLE api_keys (
    key_sha256   bytea PRIMARY KEY,
    user_sub     text NOT NULL REFERENCES users(sub) ON DELETE CASCADE,
    label        text NOT NULL,
    prefix       text NOT NULL,
    created_at   timestamptz NOT NULL DEFAULT now(),
    last_used_at timestamptz,
    revoked_at   timestamptz
);

-- A revoked key stays in the table: `last_used_at` keeps its evidential value.
CREATE INDEX api_keys_user_sub_idx ON api_keys (user_sub) WHERE revoked_at IS NULL;
