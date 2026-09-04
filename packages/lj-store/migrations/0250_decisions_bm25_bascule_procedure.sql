-- Migration 0250. Bascule sur l'index construit par la 0249.
--
-- Même geste que la 0246 : DROP + RENAME transactionnels sous le lock_timeout de
-- 5 s du migrator, le nom `decisions_bm25` rendu dans la même transaction. Le
-- _v4 est un surensemble du _v3, donc le serveur déployé travaille dessus sans
-- rien savoir, et un échec laisse l'ancien index en service.

DROP INDEX decisions_bm25;

ALTER INDEX decisions_bm25_v4 RENAME TO decisions_bm25;
