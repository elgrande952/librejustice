-- 0224 : trois juridictions ultramarines portaient un nom qu'elles n'ont pas.
--
-- Le vocabulaire fermé de `split_name` ignorait leurs formes ; le type n'étant
-- pas reconnu, le label se repliait sur le type nu et perdait la ville. Papeete
-- et Nouméa sont devenues « Tribunal judiciaire », qui n'existe pas outre-mer ;
-- Fort-de-France a perdu son caractère mixte. Vérifié sur pièces : les en-têtes
-- des décisions elles-mêmes disent « tribunal mixte de commerce de
-- Fort-de-France » (~700), « tribunal de première instance de Nouméa » (586) et,
-- pour Papeete, les deux graphies de la même juridiction (« civil » ou non).
--
-- `ensure_jurisdictions` gèle le label dès qu'une ville existe : l'extraction
-- corrigée ne repasse pas dessus, d'où cette réparation à la main. Le label ne
-- vit que dans `jurisdiction` ; `decisions.search_title` est reconstruit par
-- `reextract-fields`, qui resynchronise les chunks par trigger.

UPDATE jurisdiction SET label = 'Tribunal de première instance de Papeete'
 WHERE code = 'tj_papeete';
UPDATE jurisdiction SET label = 'Tribunal de première instance de Nouméa'
 WHERE code = 'tj_noumea';
UPDATE jurisdiction SET label = 'Tribunal mixte de commerce de Fort-de-France'
 WHERE code = 'tcom_fort_de_france';
