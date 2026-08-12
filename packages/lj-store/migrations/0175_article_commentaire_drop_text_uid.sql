-- Contraction de l'ancrage par slug (expansion en 0174) : l'app qui
-- lisait `article_commentaire.text_uid` est remplacée, la colonne et son index
-- de lecture n'ont plus de lecteur. Une seule vérité d'ancre (#11).
DROP INDEX IF EXISTS article_commentaire_anchor_idx;
ALTER TABLE article_commentaire DROP COLUMN IF EXISTS text_uid;
