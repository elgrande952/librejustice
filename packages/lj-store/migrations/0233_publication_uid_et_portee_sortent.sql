-- La publication se filtre sur `publication_codes` ; le rang le plus fort
-- recalculé à l'ingest n'a plus de lecteur. Sa FK tenait la ligne « Autre », que
-- 2,5 M décisions désignaient : elle part avec la colonne, pas avant.

ALTER TABLE decisions DROP COLUMN publication_uid;

DELETE FROM facet_value WHERE uid = 'publication:AUTRE';
DELETE FROM facet_value WHERE facet = 'significance';
