CREATE TABLE gemini_key_day (
    fingerprint text    NOT NULL,
    model       text    NOT NULL,
    quota_day   date    NOT NULL,
    spent       integer NOT NULL DEFAULT 0,
    updated_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (fingerprint, model, quota_day)
);
