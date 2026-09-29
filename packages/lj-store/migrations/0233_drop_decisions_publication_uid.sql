ALTER TABLE decisions DROP COLUMN publication_uid;

DELETE FROM facet_value WHERE uid = 'publication:AUTRE';
DELETE FROM facet_value WHERE facet = 'significance';
