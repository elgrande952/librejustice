-- Migration 0189 — Bascule sur l'index construit par la 0187.
--
-- Transactionnelle et brève : les deux DDL sont instantanées, seul le verrou
-- compte. `DROP INDEX` prend un ACCESS EXCLUSIVE sur legal_article ; le
-- `lock_timeout = '5s'` du migrator fait échouer la bascule plutôt
-- que de mettre en file toutes les requêtes suivantes derrière l'attente. Un
-- échec ici est sans conséquence : l'ancien index sert encore, le _v2 attend,
-- on rejoue.

DROP INDEX legal_article_bm25;

ALTER INDEX legal_article_bm25_v2 RENAME TO legal_article_bm25;
