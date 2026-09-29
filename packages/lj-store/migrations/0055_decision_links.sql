CREATE TABLE decision_links (
    src_decision_id BIGINT   NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    dst_decision_id BIGINT   NULL     REFERENCES decisions(id) ON DELETE SET NULL,
    dst_key         TEXT     NULL,
    kind            SMALLINT NOT NULL,

    CONSTRAINT decision_links_target_present
        CHECK (dst_decision_id IS NOT NULL OR dst_key IS NOT NULL)
);

ALTER TABLE decision_links
    ADD CONSTRAINT decision_links_pk
    UNIQUE NULLS NOT DISTINCT (src_decision_id, kind, dst_decision_id, dst_key);

CREATE INDEX idx_decision_links_src ON decision_links (src_decision_id, kind);

CREATE INDEX idx_decision_links_dst ON decision_links (dst_decision_id, kind)
    WHERE dst_decision_id IS NOT NULL;

CREATE INDEX idx_decision_links_dst_key ON decision_links (dst_key)
    WHERE dst_decision_id IS NULL;

ALTER TABLE decisions
    ADD COLUMN IF NOT EXISTS links_version smallint;
