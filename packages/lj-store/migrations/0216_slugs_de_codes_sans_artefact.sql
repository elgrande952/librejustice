-- Migration 0216 — Le slug d'un code ne porte pas la trace de sa fabrication
--.
--
-- Douze codes FR portaient dans leur URL publique un artefact du calcul de leur
-- slug : l'acronyme du `TITREFULL` de LEGI, que la base ne porte plus
-- (`code-general-des-impots-cgi` pour « Code général des impôts »), ou l'ordinal
-- de la cascade anti-collision (`code-penal-2`, qui ne dit pas « ancien »). Un
-- appelant ne les devine pas : les cinq du CGI ont fait 9 des ~30 erreurs MCP du
-- 11 au 14/08/2026.
--
-- Même geste que la 0141 : une table de renommages explicite, relue ligne à
-- ligne, chaque cible vérifiée libre. Ce qui n'est pas un artefact ne bouge
-- pas — `code-rural` est plus court que « Code rural et de la pêche maritime »
-- mais reste un nom que l'on tape, et les quatre fantômes
-- `code-general-des-collectivites-territoriales-2999-01-01*` visent tous un slug
-- déjà pris par le vrai CGCT.
--
-- Part APRÈS le déploiement de son code : le serveur en place doit
-- déjà savoir suivre `legal_text_slug_redirect`, sans quoi les ~39 k URL
-- indexées des anciens slugs répondent 404 le temps de la bascule.

WITH renames(ancien, nouveau) AS (VALUES
  -- Suffixe acronyme du TITREFULL de LEGI, absent du titre porté en base.
  ('code-general-des-impots-cgi',                 'code-general-des-impots'),
  ('code-general-des-impots-annexe-i-cgiani',     'code-general-des-impots-annexe-i'),
  ('code-general-des-impots-annexe-ii-cgianii',   'code-general-des-impots-annexe-ii'),
  ('code-general-des-impots-annexe-iii-cgianiii', 'code-general-des-impots-annexe-iii'),
  ('code-general-des-impots-annexe-iv-cgianiv',   'code-general-des-impots-annexe-iv'),
  -- Ordinal de collision : le titre dit « (ancien) » / « (nouveau) », le slug
  -- disait « -2 ».
  ('code-de-commerce-2',          'code-de-commerce-ancien'),
  ('code-penal-2',                'code-penal-ancien'),
  ('code-de-la-route-2',          'code-de-la-route-ancien'),
  ('code-de-justice-militaire-2', 'code-de-justice-militaire-nouveau'),
  -- Quatre éditions du même code : l'ordinal ne dit pas laquelle. L'édition 2006
  -- garde le slug nu, c'est celle que l'on cherche par défaut.
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

-- `legal_article.slug` est la copie dénormalisée de la 0165, rafraîchie en fin
-- d'ingest : sans cette passe les jambes articles de /textes cherchent un slug
-- qui n'existe plus, jusqu'au prochain ingest. ~39 k lignes, pas de FK.
UPDATE legal_article a SET slug = t.slug
  FROM legal_text t
 WHERE t.text_uid = a.text_uid
   AND t.text_uid IN (SELECT text_uid FROM legal_text_slug_redirect)
   AND a.slug IS DISTINCT FROM t.slug;
