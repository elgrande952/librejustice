-- Renvois des analyses AJCE du Conseil d'État. Pas des `case_citation` : celles-là
-- sont clées sur un span du corps et réécrites en entier à chaque ré-extraction,
-- là où un renvoi n'a pas de position et vit dans le payload source. Mesuré :
-- 9 702 couples, dont 59 déjà connus du corps.

CREATE TABLE decision_renvoi (
    decision_id        int8 NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    renvoi_no          int2 NOT NULL,  -- rang dans le bundle : regroupe les numéros co-cités d'un même renvoi
    target_ref         text NOT NULL,  -- `ce|335033`, `tc|00012` ; suffixe vide = renvoi à la seule page du Lebon
    target_date        date,
    nom                text,           -- nom d'usage porté par le renvoi, absent quand la source l'a caviardé
    target_decision_id int8 REFERENCES decisions(id) ON DELETE SET NULL,
    PRIMARY KEY (decision_id, renvoi_no, target_ref)
);
-- Descente : qui renvoie à cette décision ?
CREATE INDEX decision_renvoi_cible_idx
    ON decision_renvoi (target_decision_id) WHERE target_decision_id IS NOT NULL;
