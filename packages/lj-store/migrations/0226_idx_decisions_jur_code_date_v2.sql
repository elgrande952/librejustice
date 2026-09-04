-- lj:sans-transaction
-- Migration 0226 : l'index des hubs juridiction, sans sa clause morte.
--
-- `idx_decisions_jur_code_date` prédicate `public_id IS NOT NULL` depuis la
-- 0167, et la 0208/0209 a passé la colonne en NOT NULL. Postgres 17+ retire des
-- quals un `IS NOT NULL` porté par une colonne NOT NULL, et l'appariement d'un
-- index partiel ne prouve son prédicat que sur les quals restants : plus aucune
-- requête ne peut choisir cet index, même `enable_seqscan` et
-- `enable_bitmapscan` coupés ensemble. 158 Mo entretenus à chaque écriture pour
-- rien, et l'histogramme des hubs en Parallel Seq Scan des 3,73 M lignes.
--
-- Seul `deleted_at IS NULL` filtre encore, la colonne étant nullable.
--
-- Hors transaction, donc sans rollback : une interruption laisse un index
-- INVALIDE. Reprise = DROP INDEX idx_decisions_jur_code_date_v2, puis rejouer.

CREATE INDEX CONCURRENTLY idx_decisions_jur_code_date_v2
ON decisions (jurisdiction_code, date_lecture DESC, id)
WHERE deleted_at IS NULL;
