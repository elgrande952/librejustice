-- Migration 0196 : borne de validité des sessions web — étape « expand ».
--
-- Support de la déconnexion « partout, tout de suite ». Supabase révoque les
-- refresh tokens mais PAS les access tokens : ce sont des JWT validés par
-- signature, donc invalidables seulement par expiration (1 h par défaut). Un
-- autre appareil continuerait donc de fonctionner jusqu'à une heure après une
-- déconnexion globale.
--
-- On ne subit pas cette latence parce qu'on valide les JWT nous-mêmes
-- (`lj_api::auth`) : un jeton émis avant `sessions_valid_from` est refusé à la
-- porte, sans attendre son expiration. Et c'est suffisant — le front n'instancie
-- qu'`AuthClient`, pas `createClient` : Supabase ne sert qu'à l'authentification,
-- toute la donnée passe par notre API.
--
-- Pas de table de sessions, donc rien à tracer sur les appareils de
-- l'utilisateur : une seule borne par compte, et tout ce qui la précède tombe.
--
-- NULL = jamais révoqué, l'immense majorité des lignes. Pas de DEFAULT now() :
-- ça invaliderait toutes les sessions en cours à l'application de la migration.
--
-- AUCUN code ne lit encore cette colonne : la lecture (`iat` comparé à cette
-- borne) et l'écriture (endpoint de déconnexion globale) partent en release
-- suivante, quand la colonne sera déjà là. Même découpage qu'en 0195, pour la
-- même raison — le deploy précède la migration.

ALTER TABLE users ADD COLUMN sessions_valid_from TIMESTAMPTZ;

COMMENT ON COLUMN users.sessions_valid_from IS
    'Tout JWT Supabase dont iat est antérieur est refusé. NULL = jamais révoqué. '
    'Posé par la déconnexion globale ; il n''existe pas de table de sessions.';
