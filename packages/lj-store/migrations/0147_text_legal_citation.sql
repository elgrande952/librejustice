CREATE TABLE text_legal_citation (
    owner_text_uid   text NOT NULL REFERENCES legal_text(text_uid) ON DELETE CASCADE,
    owner_num_key    text,
    owner_date_debut date,
    char_start       int4 NOT NULL,
    char_end         int4 NOT NULL,
    ref_text_uid     text NOT NULL REFERENCES legal_text(text_uid) ON DELETE CASCADE,
    ref_num_key      text,
    extract_version  int2 NOT NULL,
    CHECK (char_end > char_start),
    CHECK ((owner_num_key IS NULL) = (owner_date_debut IS NULL))
);

CREATE UNIQUE INDEX text_legal_citation_owner_span_key
    ON text_legal_citation (owner_text_uid, COALESCE(owner_num_key, ''),
                            COALESCE(owner_date_debut, '0001-01-01'::date), char_start);
