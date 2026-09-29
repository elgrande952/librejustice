ALTER TABLE jurisdiction ADD COLUMN place text;

UPDATE jurisdiction j
SET place = v.place
FROM (VALUES
    ('basse-terre', 'geo:GP'),
    ('pointe-à-pitre', 'geo:GP'),
    ('guadeloupe', 'geo:GP'),
    ('cayenne', 'geo:GF'),
    ('guyane', 'geo:GF'),
    ('fort-de-france', 'geo:MQ'),
    ('martinique', 'geo:MQ'),
    ('saint-denis de la réunion', 'geo:RE'),
    ('saint-pierre de la réunion', 'geo:RE'),
    ('la réunion', 'geo:RE'),
    ('mayotte', 'geo:YT'),
    ('nouméa', 'geo:NC'),
    ('nouvelle-calédonie', 'geo:NC'),
    ('papeete', 'geo:PF'),
    ('polynésie française', 'geo:PF'),
    ('saint-barthélemy', 'geo:BL'),
    ('saint-martin', 'geo:MF')
) AS v(city, place)
WHERE lower(j.city) = v.city;
