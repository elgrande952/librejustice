-- Migration 0207 — Bascule sur l'index construit par la 0206.
--
-- Même geste que la 0193 : DROP + RENAME transactionnels sous le lock_timeout
-- de 5 s du migrator. Le nom `legal_article_usage_bm25` est rendu
-- dans la même transaction, donc l'ancien `lj-server`, qui tourne encore
-- pendant la migration, ne voit jamais la table sans son index bm25. Un échec
-- est sans conséquence : l'ancien index sert encore, le _v2 attend, on rejoue.

DROP INDEX legal_article_usage_bm25;

ALTER INDEX legal_article_usage_bm25_v2 RENAME TO legal_article_usage_bm25;
