-- lj:sans-transaction
-- Migration 0232 : l'ordre de date du mode parcours, en index.
--
-- `/decisions` sans requête liste le fonds par date de lecture décroissante.
-- Sans index sur cet ordre, la page la plus visitée du moteur trie 3,73 M
-- lignes par appel : 1,8 s mesurés en Parallel Index Only Scan de
-- `idx_decisions_jur_code_date`, dont l'ordre commence par le code.
--
-- L'ordre inverse (tri par date croissante) n'y est pas apparié : un parcours à
-- rebours rendrait `NULLS FIRST`, et les huit décisions sans date passeraient
-- devant. Il garde donc son tri.
--
-- Hors transaction, donc sans rollback : une interruption laisse un index
-- INVALIDE. Reprise = DROP INDEX ..._actives, puis rejouer. Le nom porte le
-- prédicat, `idx_decisions_date_lecture` étant pris depuis 0026 par un btree
-- simple sur la colonne, que celui-ci ne remplace pas partout.

CREATE INDEX CONCURRENTLY idx_decisions_date_lecture_actives
ON decisions (date_lecture DESC NULLS LAST, id)
WHERE deleted_at IS NULL;
