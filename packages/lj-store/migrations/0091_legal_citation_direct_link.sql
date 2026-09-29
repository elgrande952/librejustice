CREATE TABLE legal_citation (
    id                    BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    decision_id           BIGINT NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    char_start            INT,
    char_end              INT,
    surface_text          TEXT NOT NULL,
    text_key              TEXT,
    article_key           TEXT,
    ref_text_uid          TEXT,
    ref_num_key           TEXT,
    status                TEXT NOT NULL,
    non_citable_reason    TEXT,
    antecedent_char_start INT,
    antecedent_char_end   INT,
    source                TEXT NOT NULL,
    confidence            REAL,
    extract_version       SMALLINT,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_lc_decision ON legal_citation (decision_id, char_start);

CREATE INDEX idx_lc_ref ON legal_citation (ref_text_uid) WHERE ref_text_uid IS NOT NULL;

CREATE INDEX idx_lc_text_key ON legal_citation (lower(text_key));
