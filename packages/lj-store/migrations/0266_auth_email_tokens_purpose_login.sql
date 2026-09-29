ALTER TABLE auth_email_tokens DROP CONSTRAINT auth_email_tokens_purpose_check;
ALTER TABLE auth_email_tokens ADD CONSTRAINT auth_email_tokens_purpose_check
    CHECK (purpose IN ('confirmation', 'recovery', 'connexion',
                       'email_change_ancienne', 'email_change_nouvelle'));
