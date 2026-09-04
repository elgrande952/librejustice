-- lj:sans-transaction
-- Migration 0212 — index GIN d'expression sur la clé canonique.
--
-- Index d'EXPRESSION, pas de colonne : `lj_docket_keys` étant IMMUTABLE, la
-- table n'est pas touchée. Partiel sur `deleted_at IS NULL`, comme
-- `idx_decisions_docket_numbers` qu'il complète — celui-ci sert l'affichage et
-- les filtres sur la graphie source, celui-là la recherche par numéro.
--
-- La jambe `docket_leg` doit appeler l'expression à l'identique
-- (`lj_docket_keys(d.docket_numbers) && $1`), sinon le planificateur ne
-- reconnaît pas l'index.
--
-- Hors transaction, donc sans rollback : une interruption laisse un index
-- INVALIDE que le run suivant refusera de recréer. Reprise = DROP INDEX
-- idx_decisions_docket_keys, puis rejouer.

CREATE INDEX CONCURRENTLY idx_decisions_docket_keys ON decisions
USING gin (lj_docket_keys(docket_numbers)) WHERE deleted_at IS NULL;
