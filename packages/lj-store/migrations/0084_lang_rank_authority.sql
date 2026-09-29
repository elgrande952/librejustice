CREATE FUNCTION lang_rank(fields jsonb) RETURNS int
    IMMUTABLE LANGUAGE sql AS $$
    SELECT CASE
        WHEN fields->>'languageisocode' = 'FRE'
          OR fields->>'resource_obtained_language' = 'fra'
        THEN 1 ELSE 0
    END
$$;
