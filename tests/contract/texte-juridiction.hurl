# ============================================================================
# CONTRACT TESTS: the jurisdiction segment of a text URL
#   GET /internal/norm/{juridiction}/{code}[/{num}]
#   GET /texte/{code}/… redirigé en 308 vers la forme neuve
# {{base}} = host root.
# ============================================================================

# ── The segment names who enacts, and the payload says it ───────────────────
GET {{base}}/internal/norm/fr/code-civil/1240
HTTP 200
[Asserts]
jsonpath "$.code" == "code-civil"
jsonpath "$.jurisdiction" == "FR"
jsonpath "$.numKey" == "1240"

GET {{base}}/internal/norm/fr/code-civil
HTTP 200
[Asserts]
jsonpath "$.code" == "code-civil"
jsonpath "$.jurisdiction" == "FR"
jsonpath "$.articleCount" > 2000

GET {{base}}/internal/norm/fr/code-civil/sommaire
HTTP 200
[Asserts]
jsonpath "$.tree" count > 0

# ── A section is addressed by the path of its segments ──────────────────────
GET {{base}}/internal/norm/fr/code-civil/section/livre-i/titre-v
HTTP 200
[Asserts]
jsonpath "$.path" == "livre-i/titre-v"
jsonpath "$.title" == "Titre V : Du mariage"
jsonpath "$.ancestors[0].path" == "livre-i"

GET {{base}}/norm/fr/code-civil/livre-i/titre-v
HTTP 200

GET {{base}}/norm/fr/code-civil/livre-i
HTTP 200

# ── Its DILA id redirects to that path ──────────────────────────────────────
GET {{base}}/norm/fr/code-civil/section/LEGISCTA000006117710
HTTP 308
[Asserts]
header "Location" == "/norm/fr/code-civil/livre-i/titre-v"

# ── The shape it replaces still answers: the segment is optional on the door,
# canonical on the address ──────────────────────────────────────────────────
GET {{base}}/internal/norm/code-civil/1240
HTTP 200
[Asserts]
jsonpath "$.jurisdiction" == "FR"

# ── A first segment naming no jurisdiction is read as a slug ────────────────
GET {{base}}/internal/norm/zz/code-civil
HTTP 404

# Same reading over three segments: `code-civil` becomes the article number and
# `1240` the consultation date, which is not one.
GET {{base}}/internal/norm/zz/code-civil/1240
HTTP 422

# ── The public API carries the same segment ─────────────────────────────────
GET {{base}}/api/norm/fr/code-civil/1240
HTTP 401

GET {{base}}/api/texte/fr/code-civil/1240
[Options]
location: false
HTTP 308
[Asserts]
header "Location" == "/api/norm/fr/code-civil/1240"

# ── The page redirects the old address, once, to today's slug ───────────────
GET {{base}}/texte/code-civil/1240
[Options]
location: false
HTTP 308
[Asserts]
header "Location" == "/norm/fr/code-civil/1240"

GET {{base}}/texte/code-civil
[Options]
location: false
HTTP 308
[Asserts]
header "Location" == "/norm/fr/code-civil"

GET {{base}}/texte/code-civil/1240?place=NC
[Options]
location: false
HTTP 308
[Asserts]
header "Location" == "/norm/fr/code-civil/1240?place=NC"

GET {{base}}/texte/fr/code-civil/1240/comparer/2016-10-01/2020-01-01
[Options]
location: false
HTTP 308
[Asserts]
header "Location" == "/norm/fr/code-civil/1240/compare/2016-10-01/2020-01-01"

# ── The new shape is served, not redirected ─────────────────────────────────
GET {{base}}/norm/fr/code-civil/1240
HTTP 200
[Asserts]
header "Content-Type" contains "text/html"
