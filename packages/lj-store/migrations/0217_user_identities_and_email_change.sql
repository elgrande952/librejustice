CREATE TABLE user_identities (
    provider     text        NOT NULL,
    provider_sub text        NOT NULL,
    user_sub     text        NOT NULL REFERENCES users(sub) ON DELETE CASCADE,
    linked_at    timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (provider, provider_sub)
);
CREATE INDEX user_identities_user_sub_idx ON user_identities (user_sub);

INSERT INTO user_identities (provider, provider_sub, user_sub)
SELECT 'google', google_sub, sub FROM users WHERE google_sub IS NOT NULL;

ALTER TABLE auth_email_tokens ADD COLUMN new_email text;
ALTER TABLE auth_email_tokens DROP CONSTRAINT auth_email_tokens_purpose_check;
ALTER TABLE auth_email_tokens ADD CONSTRAINT auth_email_tokens_purpose_check
    CHECK (purpose IN ('confirmation', 'recovery', 'email_change_ancienne', 'email_change_nouvelle'));
