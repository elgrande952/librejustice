-- Phase 6 : dépose de la revendication `curated`.
--
-- L'appartenance d'une fiche se lit désormais sur le namespace de son uid :
-- les fonds bulk n'émettent que des uids de fond (`JORFTEXT`, `LEGITEXT`,
-- `KALITEXT`, `BOI-`, `cir_`), les instruments et le droit étranger vivent
-- sous `INTL/`, `EU/`, `CH/`, `DZ/`… — hors d'atteinte d'un upsert de fond.
-- Le drapeau protégeait la seule zone de contention réelle (contenu curé
-- posé sur un uid DILA) ; les phases 3 à 5 l'ont vidée (6 293 instruments
-- frappés, 233 fiches étrangères et 359 stubs EUR-Lex re-frappés par
-- namespace) et les trois sondes de clôture rendent 0.
--
-- Aucun index ne porte la colonne : le DROP est une opération de métadonnée.

ALTER TABLE legal_text DROP COLUMN curated;
