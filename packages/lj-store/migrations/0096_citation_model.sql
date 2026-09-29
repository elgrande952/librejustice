CREATE TABLE citation_key (
    id             int4 GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    text_key       text NOT NULL,
    article_key    text,

    nature         int2 NOT NULL,

    jurisdiction   text,
    act_date       date,
    act_num        text,
    citability     int2 NOT NULL DEFAULT 0,
    signal_version int2 NOT NULL,

    ref_text_uid   text REFERENCES legal_text(text_uid),
    ref_num_key    text,
    link_rule      int2,

    n_decisions    int4 NOT NULL DEFAULT 0,
    UNIQUE NULLS NOT DISTINCT (text_key, article_key),
    CHECK (ref_num_key IS NULL OR ref_text_uid IS NOT NULL),
    CHECK (ref_text_uid IS NULL OR link_rule IS NOT NULL)
);
CREATE INDEX idx_ck_lower_text ON citation_key (lower(text_key));
CREATE INDEX idx_ck_ref ON citation_key (ref_text_uid, ref_num_key)
    WHERE ref_text_uid IS NOT NULL;
CREATE INDEX idx_ck_pending ON citation_key (id)
    WHERE link_rule IS NULL AND citability = 0;

DROP TRIGGER IF EXISTS dc_sync_arrays_ins ON decision_citation;
DROP TRIGGER IF EXISTS dc_sync_arrays_del ON decision_citation;
DROP FUNCTION IF EXISTS sync_citation_arrays_ins();
DROP FUNCTION IF EXISTS sync_citation_arrays_del();

DROP TABLE legal_citation;

CREATE TABLE legal_citation (
    decision_id     int8 NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    char_start      int4 NOT NULL,
    char_end        int4 NOT NULL,
    key_id          int4 NOT NULL REFERENCES citation_key(id),
    extract_version int2 NOT NULL,
    PRIMARY KEY (decision_id, char_start),
    CHECK (char_end > char_start)
);
CREATE INDEX idx_lc_key ON legal_citation (key_id, decision_id);
ALTER TABLE legal_citation SET (fillfactor = 100,
    autovacuum_vacuum_scale_factor = 0.02, autovacuum_analyze_scale_factor = 0.01);

CREATE TABLE citation_occurrence_link (
    decision_id  int8 NOT NULL,
    char_start   int4 NOT NULL,
    status       int2 NOT NULL,

    ref_text_uid text REFERENCES legal_text(text_uid),
    ref_num_key  text,
    link_rule    int2 NOT NULL,
    antecedent_char_start int4,
    antecedent_char_end   int4,
    PRIMARY KEY (decision_id, char_start),
    FOREIGN KEY (decision_id, char_start)
      REFERENCES legal_citation (decision_id, char_start) ON DELETE CASCADE,
    CHECK ((status = 0) = (ref_text_uid IS NOT NULL)),
    CHECK (ref_num_key IS NULL OR ref_text_uid IS NOT NULL),
    CHECK ((antecedent_char_start IS NULL) = (antecedent_char_end IS NULL))
);
CREATE INDEX idx_col_ref ON citation_occurrence_link (ref_text_uid, ref_num_key)
    WHERE ref_text_uid IS NOT NULL;

CREATE TABLE citation_gold_anchor (
    decision_id        int8 PRIMARY KEY REFERENCES decisions(id) ON DELETE CASCADE,
    full_text_checksum int8 NOT NULL,
    loaded_at          timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE resolution_run (
    id          int4 GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    started_at  timestamptz NOT NULL DEFAULT now(),
    finished_at timestamptz,
    n_keys      int4 NOT NULL DEFAULT 0,
    n_gained    int4 NOT NULL DEFAULT 0,
    n_lost      int4 NOT NULL DEFAULT 0,
    n_moved     int4 NOT NULL DEFAULT 0,
    by_rule     jsonb
);

CREATE TABLE resolution_run_diff (
    run_id           int4 NOT NULL REFERENCES resolution_run(id) ON DELETE CASCADE,
    key_id           int4 NOT NULL,
    old_ref_text_uid text,
    new_ref_text_uid text,
    old_ref_num_key  text,
    new_ref_num_key  text,
    change           int2 NOT NULL,
    PRIMARY KEY (run_id, key_id)
);
