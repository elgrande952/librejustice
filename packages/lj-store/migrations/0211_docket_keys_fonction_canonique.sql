-- Migration 0211 — la clé canonique d'un numéro de décision.
--
-- Un numéro réduit à ses alphanumériques minuscules : `19-20.312`, `19-20312`,
-- `1920312` et `19 20 312` donnent tous `1920312`. C'est la seule façon de
-- réunir les graphies — un RegexTokenizer tantivy ne sait que couper, jamais
-- réécrire, et le banc du 14/08 mesure les quatre patterns candidats à 0,39 au
-- mieux là où la clé fait 1,0000.
--
-- IMMUTABLE : c'est ce qui autorise l'index d'expression de la 0212, donc une
-- indexation sans colonne générée ni réécriture des 3,7 millions de lignes.
-- Le pendant côté requête est `lj_core::docket_key`, asservi au même oracle.

CREATE OR REPLACE FUNCTION lj_docket_keys(nums text[]) RETURNS text[]
LANGUAGE sql IMMUTABLE PARALLEL SAFE STRICT AS $$
  SELECT array_agg(DISTINCT k ORDER BY k)
    FROM (SELECT lower(regexp_replace(n, '[^[:alnum:]]', '', 'g')) AS k
            FROM unnest(nums) AS n) s
   WHERE k <> ''
$$;
