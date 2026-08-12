-- Le fond marchés enregistre le flux, il ne stampe plus d'état :
-- la fenêtre « ouverte » se calcule (echeance > NOW() + au moins un canal),
-- closed_at n'est plus ni lu ni écrit. Aucune ligne n'est supprimée : le fond
-- est une assiette qui s'accumule (« qui achète quoi », attributions à venir).

DROP INDEX annonce_ouverte_echeance_idx;

-- La fenêtre vivante et son tri : filtre echeance > NOW(), tri échéance ASC.
-- Plein, plus partiel : à ~300 k lignes/an de flux il reste dérisoire, et il
-- ne demande aucune compaction.
CREATE INDEX annonce_fond_echeance_idx ON annonce (fond, echeance);

-- Le DROP COLUMN closed_at part au deploy suivant (les migrations
-- s'appliquent serveur debout, et l'ancien lj-server lit encore la colonne).
