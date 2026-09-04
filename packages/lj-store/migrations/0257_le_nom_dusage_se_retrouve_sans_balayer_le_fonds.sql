-- Migration 0257. Les décisions nommées se retrouvent sans balayer le fonds.
--
-- La projection des renvois efface le nom des décisions absentes de son lot.
-- Sans index, la clause `case_name_source IN ('renvoi', 'canon')` balaie les
-- 3,7 millions de lignes pour en garder 4 493, 12 s pour zéro écriture, et le
-- `statement_timeout` de 30 s du pool tombe dès que le lot compte ses milliers
-- d'identifiants. L'index partiel ne porte que les décisions nommées, 30 333.

CREATE INDEX decisions_case_name_source_idx ON decisions (case_name_source)
    WHERE case_name_source IS NOT NULL;
