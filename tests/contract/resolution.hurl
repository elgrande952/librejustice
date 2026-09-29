# ============================================================================
# CONTRACT TESTS — résolution d'un texte : ce qui doit rester joignable
#   GET /texte/{slug}[/{article}] → 308 vers l'adresse d'aujourd'hui
# {{base}} = host root, aucun secret. Les gestes vérifiés ici ne dépendent ni
# d'un classement ni d'une donnée du jour : ils tiennent ou ils cassent, et
# ils cassent en silence — une redirection perdue rend 404 à 34 510 URL
# indexées sans qu'aucune page ne change d'aspect. D'où leur place dans le
# sous-ensemble rejoué à chaque deploy.
# ============================================================================

# ── 1. Renommage de slug : l'ancien redirige, l'article aussi ────────────────
# Trois des douze couples, un par forme de renommage : artefact de fabrication
# retiré, homonyme daté, édition numérotée. La table `legal_text_slug_redirect`
# porte les neuf autres.
GET {{base}}/texte/code-general-des-impots-cgi
HTTP 308
[Asserts]
header "Location" endsWith "/norm/fr/code-general-des-impots"

GET {{base}}/texte/code-penal-2
HTTP 308
[Asserts]
header "Location" endsWith "/norm/fr/code-penal-ancien"

GET {{base}}/texte/code-des-marches-publics-3
HTTP 308
[Asserts]
header "Location" endsWith "/norm/fr/code-des-marches-publics-edition-2001"

# La redirection porte au niveau article, sans quoi les 38 896 URL d'articles
# du CGI tombent — c'est là qu'est le volume indexé, pas sur la racine.
GET {{base}}/texte/code-general-des-impots-cgi/286
HTTP 308
[Asserts]
header "Location" endsWith "/norm/fr/code-general-des-impots/286"

# ── 2. Le slug vivant sert, lui, sans redirection ────────────────────────────
GET {{base}}/internal/norm/code-general-des-impots/286
HTTP 200
[Asserts]
jsonpath "$.code" == "code-general-des-impots"
jsonpath "$.numKey" == "286"

# ── 3. Un texte sans articles se lit à sa racine ─────────────────────────────
# 42 563 textes du fonds n'ont aucun article : leur corps est tout ce qu'il y a
# à servir. Le champ `body` disparu, ils deviennent des pages vides.
GET {{base}}/internal/norm/circulaire-imig0800044c-du-20-novembre-2008-relative-a-l-accord-franco-gabonais
HTTP 200
[Asserts]
jsonpath "$.articleCount" == 0
jsonpath "$.body" isString

# ── 4. Un code, lui, rend son identité et son compte d'articles ──────────────
GET {{base}}/internal/norm/code-du-travail
HTTP 200
[Asserts]
jsonpath "$.code" == "code-du-travail"
jsonpath "$.articleCount" > 20000
