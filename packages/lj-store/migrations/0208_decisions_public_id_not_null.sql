-- Migration 0208 : `decisions.public_id` — le CHECK, posé sans scan (1/2).
--
-- La 0004 a ajouté la colonne et son index unique PARTIEL
-- (`WHERE public_id IS NOT NULL`), forme d'une colonne remplie après coup. Le
-- prédicat n'exclut plus rien : 3 713 496 lignes sur 3 713 496 sont renseignées
-- (mesuré le 12/08/2026). Ce qui manque est la déclaration de l'invariant.
--
-- Ce qu'il protège : `public_id` est l'identité EXTERNE d'une décision — les URLs
-- et `get_decision` la cherchent par lui, tandis que les jointures internes
-- passent par `id`, qui est référencé ~30 M de fois et reste donc la clé étroite.
-- Un chemin d'insertion qui oublierait `public_id` créerait une décision
-- inatteignable, sans URL, et l'index partiel ne dirait rien. Les deux chemins
-- d'écriture la posent déjà : l'invariant tient dans le code, pas dans le schéma.
--
-- Découpé en deux migrations parce que le migrateur applique chaque fichier dans
-- UNE transaction. `ADD CONSTRAINT … NOT VALID` prend un ACCESS EXCLUSIVE bref
-- mais le garde jusqu'au COMMIT : un `VALIDATE` dans la même transaction
-- scannerait 41 Go sous verrou exclusif, soit la table bloquée plusieurs minutes.
-- Séparés, le scan de la 0209 s'exécute sous SHARE UPDATE EXCLUSIVE, qui laisse
-- passer lectures ET écritures. Même découpage qu'en 0195/0196, même raison.
--
-- `lock_timeout` parce que la demande d'ACCESS EXCLUSIVE se met en file derrière
-- toute transaction en cours, et que TOUS les lecteurs s'empilent derrière elle le
-- temps de l'attente. Échouer vite et rejouer vaut mieux qu'un gel ; le migrateur
-- pose `statement_timeout = 0`, jamais `lock_timeout`.

SET LOCAL lock_timeout = '5s';

ALTER TABLE decisions
  ADD CONSTRAINT decisions_public_id_present CHECK (public_id IS NOT NULL) NOT VALID;
