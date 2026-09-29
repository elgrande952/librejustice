CREATE TABLE cited_reference (
    id           BIGSERIAL PRIMARY KEY,
    text_key     TEXT NOT NULL,
    article_key  TEXT,
    raw_text     TEXT NOT NULL,
    raw_article  TEXT,
    ref_text_uid TEXT,
    ref_num_key  TEXT,

    UNIQUE NULLS NOT DISTINCT (text_key, article_key)
);

CREATE INDEX idx_cited_reference_text_key ON cited_reference (text_key);

CREATE INDEX idx_cited_reference_resolved
    ON cited_reference (ref_text_uid, ref_num_key)
    WHERE ref_text_uid IS NOT NULL;

CREATE TABLE decision_citation (
    decision_id        BIGINT NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    cited_reference_id BIGINT NOT NULL REFERENCES cited_reference(id) ON DELETE CASCADE,
    PRIMARY KEY (decision_id, cited_reference_id)
);

CREATE INDEX idx_decision_citation_ref ON decision_citation (cited_reference_id);
