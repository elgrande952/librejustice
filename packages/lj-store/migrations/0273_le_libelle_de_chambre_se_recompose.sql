-- Migration 0273. Le libellé de chambre cesse d'être écrit.
--
-- Le libellé est une fonction pure de `chamber_key` et `speciality_uid`, deux
-- colonnes de sa propre ligne. Le stocker faisait rendre nu, hors base, ce que
-- la base rendait qualifié : 373 faux négatifs muets au banc d'extraction.
--
-- La colonne perd sa contrainte mais reste : le serveur en place la lit encore,
-- et la beta migre avant le déploiement. La table est vidée parce que la
-- grammaire de clé change de forme (`CH:2` devient `C2`) ; `resync-chambers` la
-- reconstruit après la réextraction.

ALTER TABLE chamber ALTER COLUMN label DROP NOT NULL;
TRUNCATE TABLE chamber;
