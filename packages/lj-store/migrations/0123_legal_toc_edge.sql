CREATE TABLE legal_toc_edge (
    owner_uid     TEXT NOT NULL,
    text_uid      TEXT NOT NULL,
    seq           INTEGER NOT NULL,
    child_kind    TEXT NOT NULL,
    child_uid     TEXT NOT NULL,
    child_cid     TEXT,
    child_num_key TEXT,
    label         TEXT NOT NULL,
    etat          TEXT NOT NULL,
    date_debut    DATE,
    date_fin      DATE,
    niv           INTEGER,
    PRIMARY KEY (owner_uid, seq)
);

CREATE INDEX idx_legal_toc_edge_text ON legal_toc_edge (text_uid);

CREATE INDEX idx_legal_toc_edge_child_uid ON legal_toc_edge (child_uid);
CREATE INDEX idx_legal_toc_edge_child_cid ON legal_toc_edge (child_cid)
    WHERE child_cid IS NOT NULL;
