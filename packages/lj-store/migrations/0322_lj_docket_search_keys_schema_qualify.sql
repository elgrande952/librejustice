CREATE OR REPLACE FUNCTION lj_docket_search_keys(nums text[]) RETURNS text[]
LANGUAGE sql IMMUTABLE PARALLEL SAFE STRICT AS $$
  SELECT array_agg(DISTINCT k ORDER BY k)
    FROM (SELECT k FROM unnest(public.lj_docket_keys(nums)) AS k
          UNION ALL
          SELECT substring(k FROM '^[a-z]+([0-9]+)$') FROM unnest(public.lj_docket_keys(nums)) AS k) s
   WHERE k IS NOT NULL
$$;
