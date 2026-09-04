-- Une catégorie s'agrège plus grossièrement quand l'arrêt taît une modalité, et
-- la fourchette publiée ne s'applique alors pas. Sans cette colonne, la ligne
-- passe pour comparable à celle d'un enfant dont la minorité est établie.
ALTER TABLE quantum_observation ADD COLUMN categorie_manque text;
