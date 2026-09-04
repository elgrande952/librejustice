-- La facette juridiction se lit par blocs d'ordre, cour souveraine en tête de
-- chacun, et non par décompte, qui mettait la cour d'appel devant la Cour de
-- cassation. Judiciaire, administratif, européen, puis hors des deux ordres.

UPDATE facet_value SET sort = 1  WHERE uid = 'jurisdiction_type:CC';
UPDATE facet_value SET sort = 2  WHERE uid = 'jurisdiction_type:CA';
UPDATE facet_value SET sort = 3  WHERE uid = 'jurisdiction_type:TJ';
UPDATE facet_value SET sort = 4  WHERE uid = 'jurisdiction_type:TCOM';
UPDATE facet_value SET sort = 5  WHERE uid = 'jurisdiction_type:CE';
UPDATE facet_value SET sort = 6  WHERE uid = 'jurisdiction_type:CAA';
UPDATE facet_value SET sort = 7  WHERE uid = 'jurisdiction_type:TA';
UPDATE facet_value SET sort = 8  WHERE uid = 'jurisdiction_type:CJUE';
UPDATE facet_value SET sort = 9  WHERE uid = 'jurisdiction_type:CEDH';
UPDATE facet_value SET sort = 10 WHERE uid = 'jurisdiction_type:CONSTIT';
UPDATE facet_value SET sort = 11 WHERE uid = 'jurisdiction_type:TC';
UPDATE facet_value SET sort = 12 WHERE uid = 'jurisdiction_type:CNDA';
UPDATE facet_value SET sort = 13 WHERE uid = 'jurisdiction_type:CNIL';
