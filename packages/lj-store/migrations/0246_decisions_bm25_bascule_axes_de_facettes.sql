-- Migration 0246. Bascule sur l'index construit par la 0245.
--
-- Même geste que la 0193/0207/0214 : DROP + RENAME transactionnels sous le
-- lock_timeout de 5 s du migrator. Le nom `decisions_bm25` est rendu dans la même
-- transaction, donc l'ancien `lj-server`, qui tourne encore pendant la migration,
-- ne voit jamais la table sans son index bm25. Un échec est sans conséquence :
-- l'ancien index sert encore, le _v3 attend, on rejoue.
--
-- À l'inverse de la 0214, cette bascule ne demande rien au serveur déployé : le
-- _v3 reprend les champs de l'ancien à l'identique, tokenizers compris, et n'y
-- ajoute que des fast fields. C'est un surensemble, donc le code d'avant
-- fonctionne dessus sans le savoir, et le code d'après attend son déploiement
-- sans bloquer celle-ci.

DROP INDEX decisions_bm25;

ALTER INDEX decisions_bm25_v3 RENAME TO decisions_bm25;
