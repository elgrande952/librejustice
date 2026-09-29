CREATE TABLE user_search_history (
    id          BIGSERIAL PRIMARY KEY,
    user_sub    TEXT  NOT NULL REFERENCES users (sub) ON DELETE CASCADE,
    query       TEXT  NOT NULL,
    filters     JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_user_search_history_user_created
    ON user_search_history (user_sub, created_at DESC);
