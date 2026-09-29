INSERT INTO facet_value (uid, facet, label, sort) VALUES
    ('portee:MAJEURE',      'portee', 'Majeure',      1),
    ('portee:IMPORTANTE',   'portee', 'Importante',   2),
    ('portee:LIMITEE',      'portee', 'Limitée',      3),
    ('portee:INDETERMINEE', 'portee', 'Indéterminée', 4);

UPDATE facet_value
SET label = 'Sans mention de publication'
WHERE uid = 'publication:AUTRE';
