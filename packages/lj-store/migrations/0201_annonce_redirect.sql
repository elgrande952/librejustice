-- L'identité d'un marché est frappée une fois. Quand deux lignes
-- se découvrent être le même marché, l'une absorbe l'autre et laisse ici la
-- trace ancien → nouveau — servie en 301 sur /marche/{uid}, les pages ayant
-- pu être indexées (IndexNow, sitemap). Pas de FK sur ancien_uid : la ligne
-- absorbée est supprimée après re-parentage de ses canaux et arêtes.

CREATE TABLE annonce_redirect (
    ancien_uid text PRIMARY KEY,
    nouvel_uid text NOT NULL REFERENCES annonce(uid),
    created_at timestamptz NOT NULL DEFAULT now()
);

-- Une fusion ultérieure du nouvel uid re-pointe les redirections qui le
-- visaient (chaînes aplaties à l'écriture).
CREATE INDEX annonce_redirect_nouvel_idx ON annonce_redirect (nouvel_uid);
