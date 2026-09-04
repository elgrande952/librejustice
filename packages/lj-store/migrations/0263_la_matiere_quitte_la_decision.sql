-- Migration 0263. `decisions.matiere_uid` disparaît, la 0261 ayant libéré
-- l'index qui la retenait.
--
-- 1 253 628 valeurs sur 3 737 523 décisions actives, que plus rien n'écrit ni ne
-- lit. Les 24 lignes `facet_value` du namespace `matiere:*` restent : elles
-- nomment la spécialité de 248 chambres (`chamber.speciality_uid`).

ALTER TABLE decisions DROP COLUMN matiere_uid;
