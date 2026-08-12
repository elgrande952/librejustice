-- `decision_sources.id` est une clé de substitution que rien ne lit :
-- 0 scan sur son index depuis la création de la base, aucune FK référente, aucune
-- requête Rust qui la nomme. L'identité de la table est `source_uid` (unique
-- NOT NULL, 14,8 M scans) ; la jointure passe par `decision_id`.
--
-- Le DROP emporte la contrainte de clé primaire, son index (85 Mo, rendus tout
-- de suite) et la séquence possédée par la colonne. Les 8 o/ligne du heap ne
-- reviennent qu'à la réécriture des lignes, donc jamais en pratique sur une
-- table à 0,2 UPDATE/ligne — ce n'est pas ce qu'on vient chercher.
--
-- Applicable serveur debout : la prudence vise le DROP de ce
-- que l'ancien binaire lit, or aucune requête ne nomme `id` et il n'existe
-- aucun `SELECT *` sur cette table.

ALTER TABLE decision_sources DROP COLUMN id;
