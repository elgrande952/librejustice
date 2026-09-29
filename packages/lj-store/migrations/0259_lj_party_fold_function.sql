CREATE OR REPLACE FUNCTION lj_party_fold(x text) RETURNS text
LANGUAGE sql IMMUTABLE PARALLEL SAFE STRICT AS $$
  SELECT regexp_replace(
           translate(lower(x),
                     'àáâãäåçèéêëìíîïñòóôõöøùúûüýÿ',
                     'aaaaaaceeeeiiiinoooooouuuuyy'),
           '[^a-z0-9]', '', 'g')
$$;
