CREATE TABLE llm_credential (
    fingerprint    text PRIMARY KEY,
    provider       text NOT NULL,
    purpose        text NOT NULL,
    label          text NOT NULL,
    kind           text NOT NULL CHECK (kind IN ('api_key', 'oauth')),
    secret         jsonb NOT NULL,
    expires_at     timestamptz,
    disabled_until timestamptz,
    last_status    smallint,
    marked_by      text,
    rotated_at     timestamptz NOT NULL DEFAULT now(),
    created_at     timestamptz NOT NULL DEFAULT now()
);

COMMENT ON COLUMN llm_credential.fingerprint IS 'xxh3-64 hex of the secret, the same one mistral_key_status keys on';
COMMENT ON COLUMN llm_credential.secret IS 'api_key: {"key"}; oauth: {"access_token","refresh_token","id_token","account_id"}';
COMMENT ON COLUMN llm_credential.rotated_at IS 'concurrency token: a refresh lands only if it cites the value it read';

CREATE INDEX llm_credential_pool ON llm_credential (provider, purpose);
