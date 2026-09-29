ALTER TABLE legal_text ADD COLUMN body text;

ALTER TABLE legal_text ADD COLUMN status text;

DROP INDEX referential_texts_title_bm25;

CREATE INDEX legal_text_body_bm25 ON legal_text
USING bm25 (
    id,
    title,
    body,
    (nature::pdb.literal),
    (jurisdiction::pdb.literal)
)
WITH (
  key_field = 'id',
  text_fields = '{
    "title": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"},
    "body":  {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"}
  }'
);

CREATE TABLE text_case_citation (
    owner_text_uid     text NOT NULL REFERENCES legal_text(text_uid) ON DELETE CASCADE,
    owner_num_key      text,
    owner_date_debut   date,
    char_start         int4 NOT NULL,
    char_end           int4 NOT NULL,
    target_ref         text NOT NULL,
    target_decision_id int8 REFERENCES decisions(id) ON DELETE SET NULL,
    extract_version    int2 NOT NULL,
    CHECK (char_end > char_start),
    CHECK ((owner_num_key IS NULL) = (owner_date_debut IS NULL))
);

CREATE UNIQUE INDEX text_case_citation_owner_span_key
    ON text_case_citation (owner_text_uid, COALESCE(owner_num_key, ''),
                           COALESCE(owner_date_debut, '0001-01-01'::date), char_start);

CREATE INDEX text_case_citation_pending_target_idx
    ON text_case_citation (target_ref) WHERE target_decision_id IS NULL;

CREATE INDEX text_case_citation_target_decision_idx
    ON text_case_citation (target_decision_id) WHERE target_decision_id IS NOT NULL;
