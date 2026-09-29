ALTER TABLE decision_sources DROP COLUMN served_lang;
ALTER TABLE decision_sources ADD COLUMN lang text;

UPDATE decision_sources SET lang = CASE
    WHEN source_fields->>'languageisocode' = 'FRE'
      OR source_fields->>'resource_obtained_language' = 'fra' THEN 'fra'
    WHEN source_fields->>'languageisocode' = 'ENG'
      OR source_fields->>'resource_obtained_language' = 'eng' THEN 'eng'
END
WHERE source IN ('cedh', 'cjue');
