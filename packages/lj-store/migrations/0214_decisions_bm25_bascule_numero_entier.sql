-- Migration 0214 — Bascule sur l'index construit par la 0213.
--
-- Même geste que la 0193/0207 : DROP + RENAME transactionnels sous le
-- lock_timeout de 5 s du migrator. Le nom `decisions_bm25` est rendu dans la même
-- transaction, donc l'ancien `lj-server`, qui tourne encore pendant la migration,
-- ne voit jamais la table sans son index bm25. Un échec est sans conséquence :
-- l'ancien index sert encore, le _v2 attend, on rejoue.
--
-- Le serveur déployé doit déjà porter le pattern de la 0213 côté requête
-- (`lj_core::body_tok::TOKEN_PATTERN`) : la migration passe APRÈS le deploy, sans
-- quoi la jambe body cherche `2008 115` dans un index qui contient `2008/115`.

DROP INDEX decisions_bm25;

ALTER INDEX decisions_bm25_v2 RENAME TO decisions_bm25;
