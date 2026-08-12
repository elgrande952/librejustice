-- Ancre des commentaires de norme : `text_slug` à côté de `text_uid` (même
-- doctrine). Le `text_uid` est une identité PHYSIQUE de source, que
-- nous ne maîtrisons pas : la CEDH est passée de `JORFTEXT000000886019` (le
-- décret de publication) à `INTL/CEDH`, et la note de cabinet ancrée dessus a
-- cessé de s'afficher sans un mot. Le slug est notre clé d'adressage (unique
-- en base), celle que le rendu détient déjà — l'ancre cesse d'être un
-- état dérivé, donc il n'y a plus rien à re-dériver ni à faire dériver.
--
-- ÉTAPE D'EXPANSION. `text_uid` reste, et son index de lecture avec : `lj-server`
-- n'applique PAS les migrations (seul `lj-ingest` le fait), donc l'app en place
-- interroge encore `ac.text_uid` au moment où cette migration passe. La retirer
-- ici, c'est un 500 sur TOUTES les pages d'article jusqu'au deploy suivant —
-- avec mise en cache edge des erreurs et 5xx servis aux crawlers. La
-- contraction (retrait de la colonne et de l'ancien index) vit dans une
-- migration ultérieure, appliquée après le remplacement de l'app.

-- Lignes inadressables : un commentaire dont l'ancre ne résout aucun texte à
-- slug ne peut ni s'afficher ni se corriger — le dataset curé le réinsérera
-- sous sa bonne ancre s'il est toujours voulu (seed réconciliant).
DELETE FROM article_commentaire
WHERE text_uid NOT IN (SELECT text_uid FROM legal_text WHERE slug IS NOT NULL);

ALTER TABLE article_commentaire ADD COLUMN IF NOT EXISTS text_slug TEXT;

UPDATE article_commentaire ac
SET text_slug = lt.slug
FROM legal_text lt
WHERE lt.text_uid = ac.text_uid AND ac.text_slug IS NULL;

ALTER TABLE article_commentaire ALTER COLUMN text_slug SET NOT NULL;
-- Les écrivains n'alimentent plus que le slug : `text_uid` doit tolérer NULL le
-- temps que la contraction le supprime.
ALTER TABLE article_commentaire ALTER COLUMN text_uid DROP NOT NULL;

-- Lecture par ancre (page article) : (text_slug, num_key), partiel sur vivant.
CREATE INDEX IF NOT EXISTS article_commentaire_slug_anchor_idx
    ON article_commentaire (text_slug, num_key)
    WHERE deleted_at IS NULL;
