ALTER TABLE decision_sources ADD COLUMN served_lang text
    GENERATED ALWAYS AS (
        CASE
            WHEN source_fields->>'languageisocode' = 'FRE'
              OR source_fields->>'resource_obtained_language' = 'fra' THEN 'fra'
            WHEN source_fields->>'languageisocode' = 'ENG'
              OR source_fields->>'resource_obtained_language' = 'eng' THEN 'eng'
        END
    ) VIRTUAL;

DROP FUNCTION lang_rank(jsonb);
