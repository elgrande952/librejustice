CREATE TABLE legal_text_slug_redirect (
    ancien_slug text PRIMARY KEY,
    text_uid text NOT NULL REFERENCES legal_text(text_uid) ON DELETE CASCADE,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX legal_text_slug_redirect_uid_idx ON legal_text_slug_redirect (text_uid);
