ALTER TABLE referential_texts RENAME TO legal_text;
ALTER TABLE legal_text RENAME COLUMN source_uid TO text_uid;
ALTER TABLE legal_text ADD COLUMN date_texte DATE;
ALTER TABLE legal_text ADD COLUMN date_publi DATE;

ALTER TABLE legal_text DROP CONSTRAINT referential_texts_source_source_uid_key;
ALTER TABLE legal_text DROP COLUMN source;
ALTER TABLE legal_text ADD CONSTRAINT legal_text_text_uid_key UNIQUE (text_uid);
ALTER INDEX idx_referential_texts_slug RENAME TO idx_legal_text_slug;
ALTER INDEX idx_referential_texts_title_key RENAME TO idx_legal_text_title_key;

ALTER TABLE referential_articles RENAME TO legal_article;
ALTER TABLE legal_article RENAME COLUMN text_source_uid TO text_uid;
ALTER TABLE legal_article ADD COLUMN position INTEGER;
ALTER TABLE legal_article ADD COLUMN source_url TEXT;

UPDATE legal_article SET date_debut = DATE '0001-01-01' WHERE date_debut IS NULL;

DELETE FROM legal_article a
USING (
    SELECT id, row_number() OVER (
        PARTITION BY text_uid, num_key, date_debut
        ORDER BY length(coalesce(texte, '')) DESC, id DESC
    ) AS rn
    FROM legal_article
) d
WHERE a.id = d.id AND d.rn > 1;

ALTER TABLE legal_article ALTER COLUMN date_debut SET NOT NULL;
ALTER TABLE legal_article DROP CONSTRAINT referential_articles_source_source_uid_key;
ALTER TABLE legal_article DROP CONSTRAINT referential_articles_pkey;
ALTER TABLE legal_article DROP COLUMN id;
ALTER TABLE legal_article ADD PRIMARY KEY (text_uid, num_key, date_debut);

DROP INDEX idx_ref_articles_at_date;
DROP INDEX idx_ref_articles_vigueur;
CREATE INDEX idx_legal_article_vigueur ON legal_article (text_uid, num_key) WHERE status = 'VIGUEUR';
CREATE INDEX idx_legal_article_reading ON legal_article (text_uid, position);

ALTER TABLE legal_article_resolution RENAME COLUMN ref_text_source_uid TO ref_text_uid;
ALTER TABLE legal_article_resolution DROP COLUMN ref_source;

CREATE INDEX idx_lar_reverse ON legal_article_resolution (ref_text_uid, num_key);
