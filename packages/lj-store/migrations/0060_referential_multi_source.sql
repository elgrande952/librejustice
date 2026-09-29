ALTER TABLE legal_codes DROP COLUMN IF EXISTS legitext;
DROP TABLE IF EXISTS legi_articles;
DROP TABLE IF EXISTS legi_codes;

CREATE TABLE referential_texts (
    id            BIGSERIAL PRIMARY KEY,
    source        TEXT NOT NULL,
    source_uid    TEXT NOT NULL,
    jurisdiction  TEXT NOT NULL,
    slug          TEXT,
    title         TEXT NOT NULL,
    title_key     TEXT NOT NULL,
    nature        TEXT NOT NULL,
    last_modified DATE,
    UNIQUE (source, source_uid)
);

CREATE INDEX idx_referential_texts_slug ON referential_texts (slug) WHERE slug IS NOT NULL;
CREATE INDEX idx_referential_texts_title_key ON referential_texts (title_key);

CREATE INDEX referential_texts_title_bm25 ON referential_texts
USING bm25 (id, (title::pdb.simple('stopwords_language=French', 'ascii_folding=true')))
WITH (key_field = 'id');

CREATE TABLE referential_articles (
    id               BIGSERIAL PRIMARY KEY,
    source           TEXT NOT NULL,
    source_uid       TEXT NOT NULL,
    text_source_uid  TEXT NOT NULL,
    num              TEXT NOT NULL,
    num_key          TEXT NOT NULL,
    title_path       TEXT,
    status           TEXT NOT NULL,
    date_debut       DATE,
    date_fin         DATE,
    texte            TEXT,
    nota             TEXT,
    content_checksum BIGINT NOT NULL,
    UNIQUE (source, source_uid)
);

CREATE INDEX idx_ref_articles_at_date
    ON referential_articles (source, text_source_uid, num_key, date_debut);

CREATE INDEX idx_ref_articles_vigueur
    ON referential_articles (source, text_source_uid, num_key)
    WHERE status = 'VIGUEUR';

ALTER TABLE legal_codes ADD COLUMN referential_text_id BIGINT REFERENCES referential_texts(id);

ALTER TABLE legal_articles ADD COLUMN num_key TEXT;
CREATE INDEX idx_legal_articles_num_key ON legal_articles (code_id, num_key);

CREATE TABLE legal_article_resolution (
    article_id          BIGINT PRIMARY KEY REFERENCES legal_articles(id) ON DELETE CASCADE,
    ref_source          TEXT NOT NULL,
    ref_text_source_uid TEXT NOT NULL,
    num_key             TEXT NOT NULL
);

CREATE INDEX idx_lar_reverse
    ON legal_article_resolution (ref_source, ref_text_source_uid, num_key);
