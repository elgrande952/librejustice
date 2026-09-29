CREATE TABLE annuaire_registre (

    category text PRIMARY KEY,
    total    bigint NOT NULL
);

INSERT INTO annuaire_registre (category, total)
SELECT CASE
         WHEN uid LIKE 'siren:%' AND nature = 'morale_privee'
           THEN 'entreprises'
         WHEN uid LIKE 'siren:%' AND nature = 'morale_publique'
           THEN 'personnes_publiques'
         WHEN uid LIKE 'rna:%' THEN 'associations'
         WHEN uid LIKE 'cnb:%' OR uid LIKE 'oacc:%' THEN 'avocats'
       END AS category,
       count(*)
FROM entity
WHERE uid LIKE 'siren:%' OR uid LIKE 'rna:%'
   OR uid LIKE 'cnb:%' OR uid LIKE 'oacc:%'
GROUP BY 1;
