-- La facette publication se lit du plus fort au plus faible, par ordre. Le
-- rapport annuel passe devant le bulletin : il en est la sélection, et les
-- paliers d'autorité de lj-core le classaient déjà au-dessus.

UPDATE facet_value SET sort = 1 WHERE uid = 'publication:PUBLIE_RAPPORT';
UPDATE facet_value SET sort = 2 WHERE uid = 'publication:PUBLIE_BULLETIN';
