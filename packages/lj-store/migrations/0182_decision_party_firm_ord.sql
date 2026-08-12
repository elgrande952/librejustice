-- Apposition avocat ↔ cabinet lue aux spans.
--
-- `firm_ord` : l'`ord` de la ligne `law_firm` de la MÊME décision apposée au
-- conseil (« Me X de la SELARL Y »). Référence de ligne intra-table, sans FK :
-- `decision_party` est réécrite par DELETE + COPY au grain décision, la
-- cohérence vit chez l'écrivain qui pose les deux lignes dans le même passage.
ALTER TABLE decision_party ADD COLUMN firm_ord int;

-- Le dériveur d'affiliations parcourt les couples appariés : (entity_uid non
-- nul, firm_ord non nul) est la sélection utile, très minoritaire dans la
-- table (~10 M lignes).
CREATE INDEX decision_party_firm_ord_idx ON decision_party (decision_id, firm_ord)
    WHERE firm_ord IS NOT NULL;
