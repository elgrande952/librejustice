CREATE TABLE annonce_redirect (
    ancien_uid text PRIMARY KEY,
    nouvel_uid text NOT NULL REFERENCES annonce(uid),
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX annonce_redirect_nouvel_idx ON annonce_redirect (nouvel_uid);
