-- Migration 0252 : `fastupdate=off` sur l'index de clés de docket, comme
-- `idx_decisions_docket_numbers` sur la même colonne. Sur un index
-- d'EXPRESSION, la liste d'attente oblige chaque recherche par numéro à
-- réévaluer `lj_docket_keys` sur les tuples pas encore indexés : 13 195 en
-- attente mesurés sur la prod, 36 h après le dernier autovacuum de `decisions`.

ALTER INDEX idx_decisions_docket_keys SET (fastupdate = off);

SELECT gin_clean_pending_list('idx_decisions_docket_keys');
