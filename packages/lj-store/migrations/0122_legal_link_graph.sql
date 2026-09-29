CREATE TABLE legal_link (
    owner_text_uid   TEXT NOT NULL,
    owner_num_key    TEXT NOT NULL,
    owner_date_debut DATE NOT NULL,
    seq              INTEGER NOT NULL,

    typelien         TEXT NOT NULL,
    verb             TEXT NOT NULL,
    direction        TEXT NOT NULL,

    target_kind      TEXT NOT NULL,
    target_uid       TEXT,
    target_text_uid  TEXT,
    target_num       TEXT,
    target_num_key   TEXT,
    target_nature    TEXT,
    target_label     TEXT NOT NULL,
    target_date      DATE,
    target_nor       TEXT,
    PRIMARY KEY (owner_text_uid, owner_num_key, owner_date_debut, seq)
);

CREATE INDEX idx_legal_link_target_uid ON legal_link (target_uid)
    WHERE target_uid IS NOT NULL;
CREATE INDEX idx_legal_link_target_art ON legal_link (target_text_uid, target_num_key)
    WHERE target_text_uid IS NOT NULL;

CREATE INDEX idx_legal_article_source_uid ON legal_article (source_uid);
