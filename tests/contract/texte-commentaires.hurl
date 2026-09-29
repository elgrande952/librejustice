# ============================================================================
# CONTRACT TESTS: doctrine commentaries carried by a text
#   GET /internal/norm/{slug} -> $.commentaires
#   GET /norm/fr/{slug}       -> the rendered block
# {{base}} = host root. The bundle is anchored on the whole text, which is
# what a law commentary comments; an article keeps the anchor on its number.
# The HAL harvest only grows, so a count is asserted as a floor.
# ============================================================================

# Sapin II: 9 HAL deposits name the law in their title on 21/09/2026.
GET {{base}}/internal/norm/loi-n-2016-1691-du-9-decembre-2016-relative-a-la-transparence-a-la-lutte-contre
HTTP 200
[Asserts]
jsonpath "$.commentaires" count >= 5
jsonpath "$.commentaires[0].kind" == "note"
jsonpath "$.commentaires[0].title" isString
jsonpath "$.commentaires[0].url" startsWith "https://"
# The edition reference travels: 22 308 of the 26 439 decision links and 693 of
# the 838 law ones carry a page, which is what makes a note citable.
jsonpath "$.commentaires[?(@.pages)]" count >= 5

# An uncommented text carries no empty bundle: the key is absent, and the
# front reads that absence rather than an empty list.
GET {{base}}/internal/norm/code-du-travail
HTTP 200
[Asserts]
jsonpath "$.commentaires" not exists

# The page renders what the API serves, outbound links included. The slug is
# the canonical one, the bare form answering 308.
GET {{base}}/norm/fr/loi-n-2016-1691-du-9-decembre-2016-relative-a-la-transparence-a-la-lutte-contre
HTTP 200
[Asserts]
header "Content-Type" contains "text/html"
body contains "Commentaires"
body contains "hal.science/hal-01612976"
