ALTER TABLE user_search_history
    ADD COLUMN source TEXT NOT NULL DEFAULT 'web';

CREATE TABLE user_decision_views (
    user_sub        TEXT    NOT NULL REFERENCES users (sub) ON DELETE CASCADE,
    decision_id     BIGINT  NOT NULL REFERENCES decisions (id) ON DELETE CASCADE,
    view_count      INTEGER NOT NULL DEFAULT 1,
    last_source     TEXT    NOT NULL DEFAULT 'web',
    first_viewed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_viewed_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_sub, decision_id)
);

CREATE INDEX idx_user_decision_views_user_viewed
    ON user_decision_views (user_sub, last_viewed_at DESC);
