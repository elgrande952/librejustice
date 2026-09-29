# Smoke test — endpoints avec DB (staging uniquement)
# Usage : hurl --variable api_url=https://librejustice.fr tests/smoke/search.hurl --test

GET {{api_url}}/internal/search?q=tribunal+administratif&limit=3
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection
jsonpath "$.allHitPaths" isCollection

GET {{api_url}}/internal/search/decompte?q=tribunal+administratif&limit=3
HTTP 200
[Asserts]
jsonpath "$.total" isInteger
jsonpath "$.total" >= 0
