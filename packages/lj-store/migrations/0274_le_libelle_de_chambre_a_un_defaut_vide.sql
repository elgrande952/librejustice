-- Migration 0274. `chamber.label` prend un défaut vide.
-- Le serveur en place lit encore la colonne, rechargée toutes les heures : un
-- nul le met en erreur dès que `resync-chambers` repeuple la table.

ALTER TABLE chamber ALTER COLUMN label SET DEFAULT '';
