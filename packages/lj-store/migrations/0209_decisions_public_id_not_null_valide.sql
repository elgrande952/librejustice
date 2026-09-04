-- Migration 0209 : `decisions.public_id` NOT NULL — validation et bascule (2/2).
--
-- Suite de la 0208, dans une transaction distincte : c'est ce qui fait que le scan
-- de validation ne bloque pas la table (cf. l'en-tête de la 0208).
--
-- `VALIDATE CONSTRAINT` scanne les 41 Go sous SHARE UPDATE EXCLUSIVE : lectures et
-- écritures continuent. Puis `SET NOT NULL` s'appuie sur ce CHECK désormais valide
-- et ne refait AUCUN scan (PG 12+) — son ACCESS EXCLUSIVE ne dure donc que le
-- temps d'une écriture de catalogue. Le CHECK devient redondant et part.
--
-- À appliquer hors d'une phase d'ingest qui écrit sur `decisions` (le résumé fait
-- des UPDATE) : les deux derniers ALTER prennent un ACCESS EXCLUSIVE, qui
-- attendrait la fin de ces transactions. Avec `lock_timeout`, l'attente échoue et
-- la migration est rejouable au lieu de geler le site.

SET LOCAL lock_timeout = '5s';

ALTER TABLE decisions VALIDATE CONSTRAINT decisions_public_id_present;

ALTER TABLE decisions ALTER COLUMN public_id SET NOT NULL;

ALTER TABLE decisions DROP CONSTRAINT decisions_public_id_present;
