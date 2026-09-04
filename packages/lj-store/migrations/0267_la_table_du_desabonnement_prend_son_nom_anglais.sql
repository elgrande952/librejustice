-- Migration 0267 : le nom suit la convention du schéma des comptes, anglais
-- partout ailleurs, et celle des routes de compte.

ALTER TABLE comptes.desabonnements RENAME TO unsubscribes;
ALTER INDEX comptes.desabonnements_pkey RENAME TO unsubscribes_pkey;
