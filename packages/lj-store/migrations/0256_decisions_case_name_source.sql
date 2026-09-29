ALTER TABLE decisions
    ADD COLUMN case_name_source text
        CHECK (case_name_source IN ('canon', 'ce-list', 'source', 'renvoi'));

CREATE FUNCTION lj_case_name_rank(src text) RETURNS int
LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
    SELECT CASE src
        WHEN 'canon' THEN 3
        WHEN 'ce-list' THEN 2
        WHEN 'source' THEN 1
        WHEN 'renvoi' THEN 0
        ELSE -1
    END
$$;
