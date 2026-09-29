UPDATE jurisdiction SET label = 'Tribunal judiciaire de Rennes', city = 'Rennes'
WHERE code = 'tj35238';
UPDATE jurisdiction SET label = 'Tribunal judiciaire de Fort-de-France', city = 'Fort-de-France'
WHERE code = 'tj97209';
UPDATE jurisdiction SET label = 'Tribunal judiciaire d''Oloron-Sainte-Marie', city = 'Oloron-Sainte-Marie'
WHERE code = 'tj64445';
UPDATE jurisdiction SET label = 'Tribunal judiciaire de Châlons-en-Champagne', city = 'Châlons-en-Champagne'
WHERE code = 'tj51108';
UPDATE jurisdiction SET label = 'Tribunal de commerce d''Angoulême', city = 'Angoulême'
WHERE code = 'tcom1601';
UPDATE jurisdiction SET label = 'Tribunal de commerce de Chambéry', city = 'Chambéry'
WHERE code = 'tcom7301';
UPDATE jurisdiction SET label = 'Tribunal de commerce d''Évry', city = 'Évry'
WHERE code = 'tcom7801';
UPDATE jurisdiction SET label = 'Tribunal de commerce de Châlons-en-Champagne', city = 'Châlons-en-Champagne'
WHERE code = 'tcom5101';
UPDATE jurisdiction SET label = 'Tribunal administratif de Nouvelle-Calédonie', city = 'Nouvelle-Calédonie'
WHERE code = 'ta_nouvelle_caledonie';

UPDATE decisions SET jurisdiction_code = 'ta_amiens' WHERE jurisdiction_code = 'ta_d_amiens';
UPDATE decisions SET jurisdiction_code = 'caa_versailles' WHERE jurisdiction_code = 'caa_versailless';
UPDATE decisions SET jurisdiction_code = 'ta_reunion' WHERE jurisdiction_code = 'ta_la_reunion';
DELETE FROM jurisdiction WHERE code IN ('ta_d_amiens', 'caa_versailless', 'ta_la_reunion');

UPDATE decisions d SET jurisdiction_name = j.label
FROM jurisdiction j
WHERE j.code = d.jurisdiction_code
  AND d.jurisdiction_code IN (
    'tj35238', 'tj97209', 'tj64445', 'tj51108',
    'tcom1601', 'tcom7301', 'tcom7801', 'tcom5101',
    'ta_nouvelle_caledonie', 'ta_amiens', 'caa_versailles', 'ta_reunion'
  )
  AND d.jurisdiction_name IS DISTINCT FROM j.label;

UPDATE decision_chunks c
SET jurisdiction_name = d.jurisdiction_name, search_title = d.search_title
FROM decisions d
WHERE d.id = c.decision_id
  AND d.jurisdiction_code IN (
    'tj35238', 'tj97209', 'tj64445', 'tj51108',
    'tcom1601', 'tcom7301', 'tcom7801', 'tcom5101',
    'ta_nouvelle_caledonie', 'ta_amiens', 'caa_versailles', 'ta_reunion'
  )
  AND (c.jurisdiction_name IS DISTINCT FROM d.jurisdiction_name
       OR c.search_title IS DISTINCT FROM d.search_title);
