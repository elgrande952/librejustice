-- Migration 0221 — Bascule sur les trois index construits par les 0218 à 0220.
--
-- Même geste que la 0214 : DROP + RENAME transactionnels sous le lock_timeout de
-- 5 s du migrator. Les trois noms sont rendus dans la même transaction, donc le
-- moteur `/textes`, dont les cinq jambes interrogent les trois index ensemble, ne
-- voit jamais l'un basculé et les autres pas. Un échec est sans conséquence : les
-- anciens index servent encore, les _v2 attendent, on rejoue.
--
-- Le serveur déployé doit déjà porter le découpage de `conj_title_query` aligné
-- sur le nouveau pattern : la migration passe APRÈS le deploy, sans quoi la
-- clause conjonctive cherche `2 1` dans un index qui contient `2.1`.

DROP INDEX legal_article_bm25;
DROP INDEX legal_text_body_bm25;
DROP INDEX legal_article_usage_bm25;

ALTER INDEX legal_article_bm25_v2 RENAME TO legal_article_bm25;
ALTER INDEX legal_text_body_bm25_v2 RENAME TO legal_text_body_bm25;
ALTER INDEX legal_article_usage_bm25_v2 RENAME TO legal_article_usage_bm25;
