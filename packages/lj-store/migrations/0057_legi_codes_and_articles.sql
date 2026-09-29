CREATE TABLE legi_codes (
    legitext              TEXT PRIMARY KEY,
    code_court            TEXT,
    titre                 TEXT NOT NULL,
    nature                TEXT NOT NULL,
    derniere_modification DATE
);

CREATE INDEX idx_legi_codes_code_court ON legi_codes (code_court)
    WHERE code_court IS NOT NULL;

CREATE TABLE legi_articles (
    id               BIGSERIAL PRIMARY KEY,
    legiarti         TEXT NOT NULL UNIQUE,
    legitext         TEXT NOT NULL,
    num              TEXT NOT NULL,
    num_key          TEXT NOT NULL,
    titre_text       TEXT,
    etat             TEXT NOT NULL,
    date_debut       DATE NOT NULL,
    date_fin         DATE,
    texte            TEXT,
    nota             TEXT,
    content_checksum BIGINT NOT NULL
);

CREATE INDEX idx_legi_articles_at_date ON legi_articles (legitext, num_key, date_debut);

CREATE INDEX idx_legi_articles_vigueur ON legi_articles (legitext, num_key)
    WHERE etat = 'VIGUEUR';

ALTER TABLE legal_codes ADD COLUMN legitext TEXT REFERENCES legi_codes(legitext);
