-- Migration 0193 — Bascule sur l'index construit par la 0192.
--
-- Même geste que la 0188 : DROP + RENAME transactionnels sous le
-- lock_timeout de 5 s du migrator. Un échec est sans
-- conséquence : l'ancien index sert encore, le _v2 attend, on rejoue.

DROP INDEX legal_text_body_bm25;

ALTER INDEX legal_text_body_bm25_v2 RENAME TO legal_text_body_bm25;
