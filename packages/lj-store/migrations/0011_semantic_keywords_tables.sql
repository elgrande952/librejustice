CREATE TABLE semantic_vocabulary (
    word      TEXT PRIMARY KEY,
    embedding vector(1024) NOT NULL
);

CREATE TABLE decision_semantic_keywords (
    decision_id BIGINT PRIMARY KEY REFERENCES decisions(id) ON DELETE CASCADE,
    keywords    TEXT[] NOT NULL DEFAULT ARRAY[]::TEXT[],
    expressions TEXT[] NOT NULL DEFAULT ARRAY[]::TEXT[],
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
