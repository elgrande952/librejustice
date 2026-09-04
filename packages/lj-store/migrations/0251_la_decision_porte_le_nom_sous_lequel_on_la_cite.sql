-- Migration 0251. Le nom d'usage d'un arrêt devient une colonne de la décision.
--
-- 4 487 noms tenus par les renvois officiels du Conseil d'État contre 117 dans
-- le dataset curé qu'ils remplacent. `case_name` est la forme affichable, une
-- par décision ; `case_name_keys` porte toutes ses graphies repliées par
-- `lj_core::case_names::case_name_key`, « Tropic » valant « Société Tropic Travaux
-- Signalisation ». Les deux colonnes n'ont qu'un écrivain, le job `renvois`.

ALTER TABLE decisions
    ADD COLUMN case_name text,
    ADD COLUMN case_name_keys text[];

CREATE INDEX decisions_case_name_keys_idx ON decisions USING gin (case_name_keys)
    WHERE case_name_keys IS NOT NULL;
