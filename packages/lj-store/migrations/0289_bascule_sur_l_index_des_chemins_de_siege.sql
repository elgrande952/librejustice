-- Migration 0289. Swap onto the index built by 0286.
--
-- DROP + RENAME in one transaction, under the migrator's 5 s lock_timeout: a
-- failure leaves the old index in service.
--
-- The old code does not survive this swap: its facet aggregation names
-- `chamber_key` as a fast field, absent from `decisions_bm25_v6`, and every
-- faceted search falls to a 500 until the deployment that goes with it.

DROP INDEX decisions_bm25;

ALTER INDEX decisions_bm25_v6 RENAME TO decisions_bm25;
