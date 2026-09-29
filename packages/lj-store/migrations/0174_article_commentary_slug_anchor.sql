DELETE FROM article_commentaire
WHERE text_uid NOT IN (SELECT text_uid FROM legal_text WHERE slug IS NOT NULL);

ALTER TABLE article_commentaire ADD COLUMN IF NOT EXISTS text_slug TEXT;

UPDATE article_commentaire ac
SET text_slug = lt.slug
FROM legal_text lt
WHERE lt.text_uid = ac.text_uid AND ac.text_slug IS NULL;

ALTER TABLE article_commentaire ALTER COLUMN text_slug SET NOT NULL;

ALTER TABLE article_commentaire ALTER COLUMN text_uid DROP NOT NULL;

CREATE INDEX IF NOT EXISTS article_commentaire_slug_anchor_idx
    ON article_commentaire (text_slug, num_key)
    WHERE deleted_at IS NULL;
