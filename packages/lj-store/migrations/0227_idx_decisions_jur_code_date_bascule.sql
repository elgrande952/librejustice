-- Migration 0227 : bascule sur l'index construit par la 0226.
--
-- DROP + RENAME dans la même transaction, sous le lock_timeout de 5 s du
-- migrator : l'ancien `lj-server`, qui tourne encore pendant la migration, ne
-- voit jamais la table sans cet index. Un échec ne coûte rien, le _v2 attend.

DROP INDEX idx_decisions_jur_code_date;

ALTER INDEX idx_decisions_jur_code_date_v2
  RENAME TO idx_decisions_jur_code_date;
