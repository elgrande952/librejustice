-- Migration 0215 — Un ancien slug de texte redirige vers son texte.
--
-- Purement additive, et c'est la condition de son ordre : le binaire qui lit
-- cette table doit la trouver au démarrage, donc elle part AVANT son
-- déploiement. L'ancien serveur ne la connaît pas et s'en moque : rien ne la
-- référence tant qu'elle est vide. Les renommages qui la remplissent sont la
-- 0216, et partent APRÈS ce déploiement — un `RENAME` sur ce que le serveur en
-- place lit encore attend le déploiement suivant.

CREATE TABLE legal_text_slug_redirect (
    ancien_slug text PRIMARY KEY,
    text_uid text NOT NULL REFERENCES legal_text(text_uid) ON DELETE CASCADE,
    created_at timestamptz NOT NULL DEFAULT now()
);

-- La redirection porte l'uid, pas le slug d'arrivée : un renommage ultérieur du
-- même texte la fait suivre sans réécriture. L'index sert la lecture inverse
-- (« quels slugs pointent vers moi »).
CREATE INDEX legal_text_slug_redirect_uid_idx ON legal_text_slug_redirect (text_uid);
