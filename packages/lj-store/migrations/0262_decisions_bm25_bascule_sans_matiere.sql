-- Migration 0262. Bascule sur l'index construit par la 0261.
--
-- DROP + RENAME dans la même transaction, sous le lock_timeout de 5 s du
-- migrator : un échec laisse l'ancien index en service.

DROP INDEX decisions_bm25;

ALTER INDEX decisions_bm25_v5 RENAME TO decisions_bm25;
