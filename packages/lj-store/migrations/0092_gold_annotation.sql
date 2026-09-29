CREATE TABLE gold_annotation (
    id                BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    decision_id       BIGINT NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    version           INT  NOT NULL,
    kind              TEXT NOT NULL,
    payload           JSONB NOT NULL,
    source            TEXT NOT NULL,
    review_confidence REAL,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (decision_id, kind, version)
);

CREATE INDEX idx_gold_decision ON gold_annotation (decision_id);
