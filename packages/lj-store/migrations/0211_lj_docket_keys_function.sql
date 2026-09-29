CREATE OR REPLACE FUNCTION lj_docket_keys(nums text[]) RETURNS text[]
LANGUAGE sql IMMUTABLE PARALLEL SAFE STRICT AS $$
  SELECT array_agg(DISTINCT k ORDER BY k)
    FROM (SELECT lower(regexp_replace(n, '[^[:alnum:]]', '', 'g')) AS k
            FROM unnest(nums) AS n) s
   WHERE k <> ''
$$;
