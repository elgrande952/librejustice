-- Le référentiel auquel une observation se compare dépend du régime
-- d'indemnisation et de la gravité de la victime directe : le préjudice
-- d'affection d'un proche relève du barème du décès dès que la victime survit
-- assez gravement atteinte, renvoi que le référentiel fait lui-même. Libre en
-- jsonb, donc un élément de reclassement s'ajoute sans migration.
ALTER TABLE quantum_observation ADD COLUMN comparabilite jsonb;
