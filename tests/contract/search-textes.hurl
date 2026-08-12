# ============================================================================
# CONTRACT TESTS — GET /api/search-textes  (recherche d'articles)
# ----------------------------------------------------------------------------
# Asserts HTTP status + JSON STRUCTURAL invariants only.
# Variable: {{base}} = host root (e.g. http://localhost:8300).
# Route publique, sans auth. `q` vide ⇒ 4xx (même frontière que /api/search).
# ============================================================================

# ── 1. Baseline : recherche plein-texte d'articles ────────────────────────────
GET {{base}}/api/search-textes?q=clause%20abusive
HTTP 200
[Asserts]
header "Content-Type" contains "application/json"
jsonpath "$.total" isInteger
jsonpath "$.total" >= 0
jsonpath "$.hits" isCollection
# facets propres aux articles : code / jurisdiction / nature / source.
# L'axe code est plafonné à son top-20 (l'énumération complète pesait
# 12-19 Mo de JSON par réponse).
jsonpath "$.facets.code" isCollection
jsonpath "$.facets.code" count <= 20
jsonpath "$.facets.jurisdiction" isCollection
jsonpath "$.facets.nature" isCollection
jsonpath "$.facets.source" isCollection


# ── 2. Hit shape (assert sur le premier hit) ─────────────────────────────────
GET {{base}}/api/search-textes?q=d%C3%A9pens&limit=5
HTTP 200
[Asserts]
jsonpath "$.hits" count <= 5
jsonpath "$.hits[0].code" isString
jsonpath "$.hits[0].codeTitle" isString
jsonpath "$.hits[0].num" isString
jsonpath "$.hits[0].numKey" isString
jsonpath "$.hits[0].snippet" isString
jsonpath "$.hits[0].source" isString


# ── 2 bis. Requête citant un numéro d'article ───────────────────────────────
# Un numéro cité ouvre deux clauses SQL que rien d'autre n'emprunte : la clause
# `num` de la jambe articles et la jambe usage scopée au numéro. Une erreur de
# forme y est invisible partout ailleurs : elle rend 500 sur toute recherche
# citant un article pendant que le reste répond 200.
GET {{base}}/api/search-textes?q=L.%20423-7%20contribution&limit=5
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection

# Deux numéros, dont un nombre nu annoncé par « article » : la clause reçoit un
# tableau de clés, pas une seule.
GET {{base}}/api/search-textes?q=article%2021%20loi%2065-557%20convocation&limit=5
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection


# ── 3. q vide ⇒ 4xx (même frontière de validation que /api/search) ──────────
GET {{base}}/api/search-textes?q=
HTTP *
[Asserts]
status >= 400
status < 500


# ── 4. Filtre jurisdiction (FRANCE / UE / INTL) ──────────────────────────────
GET {{base}}/api/search-textes?q=contrat&jurisdiction=FRANCE
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection


# ── 5. Filtre nature ─────────────────────────────────────────────────────────
GET {{base}}/api/search-textes?q=travail&nature=CODE
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection


# ── 6. Filtre source ─────────────────────────────────────────────────────────
GET {{base}}/api/search-textes?q=contrat&source=legifrance
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection


# ── 7. Scoped à un code (slug) ───────────────────────────────────────────────
GET {{base}}/api/search-textes?q=d%C3%A9pens&code=code-de-procedure-civile
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection


# ── 8. limit hors bornes ⇒ 4xx ───────────────────────────────────────────────
GET {{base}}/api/search-textes?q=test&limit=101
HTTP *
[Asserts]
status >= 400
status < 500


# ── 9. Recherche à date : versions valides à cette date ─────────────────────
GET {{base}}/api/search-textes?q=clause%20abusive&date=2005-06-01
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection
jsonpath "$.total" isInteger


# ── 10. date invalide ⇒ 4xx ──────────────────────────────────────────────────
GET {{base}}/api/search-textes?q=test&date=hier
HTTP *
[Asserts]
status >= 400
status < 500
