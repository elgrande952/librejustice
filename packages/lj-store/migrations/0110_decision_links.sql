CREATE TABLE decision_links (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    decision_id BIGINT NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    link_type TEXT NOT NULL CHECK (
        link_type IN ('APPEL_DE', 'POURVOI_CONTRE', 'RENVOI_APRES_CASSATION')
    ),
    target_ref TEXT NOT NULL,
    target_decision_id BIGINT REFERENCES decisions(id) ON DELETE SET NULL,
    extract_version SMALLINT NOT NULL,
    UNIQUE (decision_id, link_type, target_ref)
);

CREATE INDEX decision_links_pending_target_idx
    ON decision_links (target_ref) WHERE target_decision_id IS NULL;

CREATE INDEX decision_links_target_decision_idx
    ON decision_links (target_decision_id) WHERE target_decision_id IS NOT NULL;
