-- Suppression du curseur de reprise (supersede 0330).
--
-- La table n'a eu qu'un appelant : le backfill BOAMP, job one-shot lancé à la
-- main sur l'hôte, absent du crontab. Chaque mois écrit une ligne
-- de journal portant `mois=` ; après une coupure, la reprise est un `--depuis`
-- sur cette valeur. Le curseur n'économisait qu'une lecture de journal, sur le
-- chemin exceptionnel d'un job qui tourne une fois.
DROP TABLE IF EXISTS ingest_cursor;
