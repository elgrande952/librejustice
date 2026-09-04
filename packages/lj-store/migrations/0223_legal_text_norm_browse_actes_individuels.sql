-- Arborescence navigable des normes : le prédicat partiel perd
-- `role <> 'individuel'`, les actes individuels JORF (nominations, promotions,
-- naturalisations) entrent dans /normes. Les expressions de fond et de date de
-- parcours sont LES MÊMES que celles de lj-store::norm_hubs (le planner ne
-- matche un index d'expression que sur l'arbre exact) : toute évolution de la
-- taxonomie = nouvelle migration qui ré-indexe.
--
-- Nouveau nom : l'index se construit à côté de l'ancien, en SHARE, les lectures
-- des pages /texte et /normes passent pendant la construction.

CREATE INDEX idx_legal_text_norm_browse ON legal_text (
  (CASE
    WHEN nature ILIKE 'code%' OR upper(nature) IN ('CODE', 'CONSTITUTION', 'ETAT_CIVIL') THEN 'codes'
    WHEN jurisdiction = 'UE' OR upper(nature) IN ('DIRECTIVE', 'DIRECTIVE_EURO', 'REGLEMENTEUROPEEN', 'REGLEMENT_EURO', 'DECISION_EURO', 'AVISEURO', 'ARRETEURO', 'ARRETEEURO', 'INSTRUCTIONEURO', 'DELIBERATIONEURO', 'DECLARATIONEURO', 'LETTREEURO') THEN 'textes-ue'
    WHEN jurisdiction = 'INTL' OR upper(nature) IN ('TRAITE', 'TI', 'PROTOCOLE') THEN 'traites'
    WHEN upper(nature) IN ('LOI', 'LOI_ORGANIQUE', 'LOI_CONSTIT', 'LOI_PROGRAMME', 'DECRET_LOI', 'ORDONNANCE') THEN 'lois'
    WHEN upper(nature) = 'DECRET' THEN 'decrets'
    WHEN upper(nature) = 'ARRETE' THEN 'arretes'
    WHEN upper(nature) IN ('IDCC', 'AVENANT', 'ACCORD', 'ACCORD_FONCTION_PUBLIQUE') THEN 'conventions-collectives'
    WHEN upper(nature) IN ('CIRCULAIRE', 'INSTRUCTION') THEN 'circulaires'
    WHEN upper(nature) = 'BOFIP' THEN 'bofip'
    ELSE 'autres'
  END),
  (CASE
    WHEN date_texte >= DATE '1500-01-01' AND date_texte < DATE '2100-01-01' THEN date_texte
    WHEN date_publi >= DATE '1500-01-01' AND date_publi < DATE '2100-01-01' THEN date_publi
  END) DESC,
  id
) WHERE slug IS NOT NULL;

DROP INDEX idx_legal_text_norm_fond_browse;
