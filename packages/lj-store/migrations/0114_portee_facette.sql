-- Portée jurisprudentielle en facette + relabel publication:AUTRE.
--
-- `portee:*` — groupes de `publication_codes` au rang le plus fort
-- (`portee_codes` de lj-core : majeure = r/A ; importante = b/l/c/B/C+/R ;
-- limitée = n/C/D/Z). Mapping total : INDETERMINEE sans code classant.
-- Pas de colonne : le filtre s'évalue en SQL sur `publication_codes`, le
-- comptage de facette en process sur le pool de candidats (comme les autres).

INSERT INTO facet_value (uid, facet, label, sort) VALUES
    ('portee:MAJEURE',      'portee', 'Majeure',      1),
    ('portee:IMPORTANTE',   'portee', 'Importante',   2),
    ('portee:LIMITEE',      'portee', 'Limitée',      3),
    ('portee:INDETERMINEE', 'portee', 'Indéterminée', 4);

-- « Autre » (repris du référentiel de place) laissait croire à une catégorie
-- éditoriale : c'est
-- l'absence de toute mention de publication dans la source (TJ/CA/TCOM, TA/CAA,
-- cours européennes…). Les référentiels de place ne les classent pas du tout.
UPDATE facet_value
SET label = 'Sans mention de publication'
WHERE uid = 'publication:AUTRE';
