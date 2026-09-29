WITH renames(ancien, nouveau) AS (VALUES

  ('code-general-des-impots-cgi',                 'code-general-des-impots'),
  ('code-general-des-impots-annexe-i-cgiani',     'code-general-des-impots-annexe-i'),
  ('code-general-des-impots-annexe-ii-cgianii',   'code-general-des-impots-annexe-ii'),
  ('code-general-des-impots-annexe-iii-cgianiii', 'code-general-des-impots-annexe-iii'),
  ('code-general-des-impots-annexe-iv-cgianiv',   'code-general-des-impots-annexe-iv'),

  ('code-de-commerce-2',          'code-de-commerce-ancien'),
  ('code-penal-2',                'code-penal-ancien'),
  ('code-de-la-route-2',          'code-de-la-route-ancien'),
  ('code-de-justice-militaire-2', 'code-de-justice-militaire-nouveau'),

  ('code-des-marches-publics-2', 'code-des-marches-publics-edition-1964'),
  ('code-des-marches-publics-3', 'code-des-marches-publics-edition-2001'),
  ('code-des-marches-publics-4', 'code-des-marches-publics-edition-2004')
),
cible AS (
  SELECT r.ancien, r.nouveau, t.text_uid
    FROM renames r JOIN legal_text t ON t.slug = r.ancien
),
trace AS (
  INSERT INTO legal_text_slug_redirect (ancien_slug, text_uid)
  SELECT ancien, text_uid FROM cible
  RETURNING text_uid
)
UPDATE legal_text t SET slug = c.nouveau
  FROM cible c WHERE t.text_uid = c.text_uid;

UPDATE legal_article a SET slug = t.slug
  FROM legal_text t
 WHERE t.text_uid = a.text_uid
   AND t.text_uid IN (SELECT text_uid FROM legal_text_slug_redirect)
   AND a.slug IS DISTINCT FROM t.slug;
