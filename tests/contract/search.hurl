# ============================================================================
# CONTRACT TESTS: GET /internal/search
# ----------------------------------------------------------------------------
# Asserts HTTP status + JSON STRUCTURAL invariants only (field presence, types,
# enum value sets, ordering contracts). NOT exact live-data values.
# Variable: {{base}} = host root (e.g. http://localhost:8300).
# The route is auth-OPTIONAL: a session changes nothing in the response.
# Facettes = référentiel (jurisdiction/legalDomain/solution/publication/
# dateLectureYear/legalInstrument).
# ============================================================================

# ── Health (sanity / smoke) ─────────────────────────────────────────────────
GET {{base}}/internal/health
HTTP 200
[Asserts]
jsonpath "$.status" == "ok"
jsonpath "$.version" isString


# ── 1. Baseline hybrid search (mode=auto, natural language → hybrid) ─────────
GET {{base}}/internal/search?q=expulsion%20locative
HTTP 200
[Asserts]
header "Content-Type" contains "application/json"
jsonpath "$.query" == "expulsion locative"
jsonpath "$.hits" isCollection
jsonpath "$.queryMode" isString
# queryMode enum set
jsonpath "$.queryMode" matches /^(lexical|hybrid|browse)$/
# auto + natural language ⇒ resolved hybrid
jsonpath "$.queryMode" == "hybrid"
jsonpath "$.allHitPaths" isCollection
# no assertion on total/facets: the count is a second call, and this response
# hands over one already resolved on its bundle, for free. Their presence
# depends on what ran before; the count route below is where the number holds.
# the ranked list itself stays capped at MAX_RESULTS
jsonpath "$.allHitPaths" count <= 100


# ── 1b. The count route: exact total + the facet axes, same query params ─────
GET {{base}}/internal/search/decompte?q=expulsion%20locative
HTTP 200
[Asserts]
header "Content-Type" contains "application/json"
jsonpath "$.total" isInteger
# total counts every matched decision, not the 100 the ranked list caps at
jsonpath "$.total" > 100
# facets struct: every dimension present as an array
jsonpath "$.facets.seat" isCollection
jsonpath "$.facets.legalDomain" isCollection
jsonpath "$.facets.solution" isCollection
jsonpath "$.facets.publication" isCollection
jsonpath "$.facets.dateLectureYear" isCollection
jsonpath "$.facets.legalInstrument" isCollection
jsonpath "$.facets.procedure" isCollection


# ── 2. Hit shape contract (only assert when at least one hit exists) ─────────
GET {{base}}/internal/search?q=responsabilite&limit=5
HTTP 200
[Asserts]
jsonpath "$.hits" count <= 5
# When hits exist, each carries the required SearchHit fields.
jsonpath "$.hits[0].id" isString
jsonpath "$.hits[0].jurisdictionType" matches /^(TA|CAA|CE|CONSTIT|TC|CC|CA|TJ|TCOM|CEDH|CJUE|CNDA)$/
jsonpath "$.hits[0].titleHtml" isString
jsonpath "$.hits[0].score" isNumber
jsonpath "$.hits[0].bestChunk" exists
jsonpath "$.hits[0].bestChunk.chunkIndex" isInteger
jsonpath "$.hits[0].bestChunk.snippet" isString
jsonpath "$.hits[0].publicationCodes" isCollection


# ── 3. mode=lexical (forces boolean_search path) ─────────────────────────────
GET {{base}}/internal/search?q=licenciement&mode=lexical
HTTP 200
[Asserts]
jsonpath "$.queryMode" == "lexical"
jsonpath "$.hits" isCollection


# ── 4. mode=semantic (forces hybrid; there is NO pure-ANN path) ──────────────
GET {{base}}/internal/search?q=trouble%20du%20voisinage&mode=semantic
HTTP 200
[Asserts]
jsonpath "$.queryMode" == "hybrid"


# ── 5. Boolean operator ET (auto-detected → lexical) ─────────────────────────
GET {{base}}/internal/search?q=cong%C3%A9s%20ET%20pay%C3%A9s
HTTP 200
[Asserts]
jsonpath "$.queryMode" == "lexical"
jsonpath "$.hits" isCollection


# ── 6. Boolean operator OU ───────────────────────────────────────────────────
GET {{base}}/internal/search?q=bail%20OU%20loyer
HTTP 200
[Asserts]
jsonpath "$.queryMode" == "lexical"


# ── 7. Boolean operator SAUF (negation) ──────────────────────────────────────
GET {{base}}/internal/search?q=cong%C3%A9s%20SAUF%20maladie
HTTP 200
[Asserts]
jsonpath "$.queryMode" == "lexical"


# ── 8. Quoted phrase (triggers BOOL_RE via the quote char → lexical) ─────────
GET {{base}}/internal/search?q=%22travail%20dominical%22
HTTP 200
[Asserts]
jsonpath "$.queryMode" == "lexical"


# ── 9. PROCHEn proximity operator → slop, lexical ────────────────────────────
GET {{base}}/internal/search?q=%22travail%20dominical%22PROCHE3
HTTP 200
[Asserts]
jsonpath "$.queryMode" == "lexical"


# ── 10. Wildcard `*` → lexical ───────────────────────────────────────────────
GET {{base}}/internal/search?q=expuls*
HTTP 200
[Asserts]
jsonpath "$.queryMode" == "lexical"


# ── 11. sort=date_desc ───────────────────────────────────────────────────────
GET {{base}}/internal/search?q=urbanisme&sort=date_desc
HTTP 200
[Asserts]
jsonpath "$.queryMode" exists
jsonpath "$.hits" isCollection


# ── 12. sort=date_asc ────────────────────────────────────────────────────────
GET {{base}}/internal/search?q=urbanisme&sort=date_asc
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection


# ── 13. Pagination: offset beyond results returns an empty page ──────────────
GET {{base}}/internal/search?q=urbanisme&offset=99&limit=1
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection
jsonpath "$.hits" count <= 1
jsonpath "$.allHitPaths" isCollection


# ── 14. Single filter: jurisdictionType (repeated query param) ────────────────
GET {{base}}/internal/search?q=contrat&jurisdictionType=CE&jurisdictionType=CAA
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection

GET {{base}}/internal/search/decompte?q=contrat&jurisdictionType=CE&jurisdictionType=CAA
HTTP 200
[Asserts]
jsonpath "$.facets.seat" isCollection


# ── 15. Single filter: solution (référentiel solution:*) ────────────────────
GET {{base}}/internal/search?q=rejet&solution=REJET
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection


# ── 16. Single filter: procedure (référentiel procedure:*) ──────────────────
GET {{base}}/internal/search?q=suspension&procedure=REFERE_SUSPENSION
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection


# ── 17. Single filter: office (référentiel office:*) ─────────────────────────
GET {{base}}/internal/search?q=saisie&office=JEX
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection


# ── 18. Single filter: legalDomain ──────────────────────────────────────────
GET {{base}}/internal/search?q=bail&legalDomain=CIVIL
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection


# ── 19. Single filter: jurisdictionCode (codes référentiel jurisdiction) ─────
GET {{base}}/internal/search?q=permis&jurisdictionCode=ce
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection


# ── 20. Single filter: legalInstrument (token = ref_text_uid) ───────────────
GET {{base}}/internal/search?q=responsabilite&legalInstrument=LEGITEXT000006070721
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection


# ── 21. Single filter: legalArticle (clé composite ref_text_uid|ref_num_key) ─
GET {{base}}/internal/search?q=responsabilite&legalArticle=LEGITEXT000006070721%7C1240
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection


# ── 22. Single filter: publication codes ─────────────────────────────────────
# Valeur canonique de facette (`facet_value.uid` sans le préfixe `publication:`),
# pas le code court de source (« B ») : un token inconnu est rejeté en 422
# correctif, jamais ignoré en silence.
GET {{base}}/internal/search?q=arret&publication=PUBLIE_BULLETIN
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection


# ── 22b. Token de filtre inconnu → 422 correctif (jamais 200 silencieux) ─────
GET {{base}}/internal/search?q=arret&publication=B
HTTP 422
[Asserts]
jsonpath "$.detail[0].loc[1]" == "publication"
jsonpath "$.detail[0].type" == "enum"


# ── 23. Date range filters ───────────────────────────────────────────────────
GET {{base}}/internal/search?q=fiscalite&dateFrom=2020-01-01&dateTo=2023-12-31
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection


# ── 24. Combined filters (many dimensions at once + date range) ──────────────
GET {{base}}/internal/search?q=expulsion&jurisdictionType=CA&solution=CONFIRMATION&legalDomain=CIVIL_DROIT_LOCATIF&dateFrom=2018-01-01&dateTo=2024-12-31&publication=PUBLIE_BULLETIN
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection

GET {{base}}/internal/search/decompte?q=expulsion&jurisdictionType=CA&solution=CONFIRMATION&legalDomain=CIVIL_DROIT_LOCATIF&dateFrom=2018-01-01&dateTo=2024-12-31&publication=PUBLIE_BULLETIN
HTTP 200
[Asserts]
jsonpath "$.facets" exists


# ── 25. aiMode=true (rerank; summary embedded if present) ────────────────────
GET {{base}}/internal/search?q=divorce&aiMode=true
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection
jsonpath "$.queryMode" exists


# ── 26. legalInstrumentFacet ordering + nested articles contract ─────────────
GET {{base}}/internal/search/decompte?q=responsabilite%20civile&limit=10
HTTP 200
[Asserts]
jsonpath "$.facets.legalInstrument" isCollection
# Each legalInstrument facet entry has value/label/count and an articles array
jsonpath "$.facets.legalInstrument[0].value" isString
jsonpath "$.facets.legalInstrument[0].label" isString
jsonpath "$.facets.legalInstrument[0].count" isInteger
jsonpath "$.facets.legalInstrument[0].articles" isCollection


# ── 27. seat facet = a tree of free depth (roots have no parent) ─────────────
GET {{base}}/internal/search/decompte?q=permis%20de%20construire&limit=10
HTTP 200
[Asserts]
jsonpath "$.facets.seat" isCollection
jsonpath "$.facets.seat[0].value" isString
jsonpath "$.facets.seat[0].label" isString
jsonpath "$.facets.seat[0].count" isInteger
# roots = a court type alone (TA, CC, …), the first segment of a seat path
jsonpath "$.facets.seat[0].value" matches /^[A-Z]{2,6}$/
jsonpath "$.facets.seat[0].parent" not exists


# ============================================================================
# ERROR / VALIDATION CONTRACTS
# Rust (parse_search_query) returns HTTP 400/422 on invalid query params —
# assert the 4xx class, not the exact code.
# ============================================================================

# ── 28. Empty q is browse mode, not an error ─────────────────────────────────
# Total and facets cover the WHOLE filtered corpus, so total goes far above the
# 100-result cap of a ranked search.
GET {{base}}/internal/search?q=&limit=5
HTTP 200
[Asserts]
jsonpath "$.queryMode" == "browse"
jsonpath "$.query" == ""
jsonpath "$.total" > 1000000
jsonpath "$.hits" count == 5
jsonpath "$.facets.seat" isCollection
jsonpath "$.facets.dateLectureYear" isCollection
# The cited-texts facet is served here too, from the nightly snapshot: the top 30
# of the whole corpus, each line carrying the catalogue title and slug.
jsonpath "$.facets.legalInstrument" count == 30
jsonpath "$.facets.legalInstrument[0].label" exists
jsonpath "$.facets.legalInstrument[0].slug" exists
jsonpath "$.facets.legalInstrument[0].articles" isCollection


# ── 29. Missing q entirely: same browse mode ─────────────────────────────────
GET {{base}}/internal/search
HTTP 200
[Asserts]
jsonpath "$.queryMode" == "browse"
jsonpath "$.total" > 1000000


# ── 29b. Browse under a filter: exact total of the filtered corpus ───────────
GET {{base}}/internal/search?jurisdictionType=CA&limit=1
HTTP 200
[Asserts]
jsonpath "$.queryMode" == "browse"
jsonpath "$.total" > 100
jsonpath "$.facets.seat" isCollection


# ── 30. limit above max (le=50) ──────────────────────────────────────────────
GET {{base}}/internal/search?q=test&limit=51
HTTP *
[Asserts]
status >= 400
status < 500


# ── 31. limit below min (ge=1) ───────────────────────────────────────────────
GET {{base}}/internal/search?q=test&limit=0
HTTP *
[Asserts]
status >= 400
status < 500


# ── 32. offset above max (le=99) ─────────────────────────────────────────────
GET {{base}}/internal/search?q=test&offset=100
HTTP *
[Asserts]
status >= 400
status < 500


# ── 33. Invalid mode enum ────────────────────────────────────────────────────
GET {{base}}/internal/search?q=test&mode=fulltext
HTTP *
[Asserts]
status >= 400
status < 500


# ── 34. Invalid sort enum ────────────────────────────────────────────────────
GET {{base}}/internal/search?q=test&sort=price
HTTP *
[Asserts]
status >= 400
status < 500


# ── 35. Unknown token on a typed facet axis (never silently unfiltered) ──────
# JAF is a valid office token, not a procedure one: axis mixups get corrected.
GET {{base}}/internal/search?q=test&procedure=JAF
HTTP *
[Asserts]
status >= 400
status < 500


# ── 36. Unknown token on a referential facet axis (never silently zero) ──────
GET {{base}}/internal/search?q=test&publication=JAF
HTTP *
[Asserts]
status >= 400
status < 500


# ── 37. Unknown jurisdictionCode (corrective error, not silently zero) ───────
GET {{base}}/internal/search?q=test&jurisdictionCode=tribunal_paris
HTTP *
[Asserts]
status >= 400
status < 500


# ── 38. A room hangs from its court type, one segment down ───────────────────
# The Cour de cassation has one court, so its code names the type: the path
# stops at two segments and the parent of a room is the root.
GET {{base}}/internal/search/decompte?q=bail%20commercial&jurisdictionCode=cc&perPage=20
HTTP 200
[Asserts]
jsonpath "$.facets.seat[?(@.value == 'CC.C3_CIVILE')]" count == 1
jsonpath "$.facets.seat[?(@.value == 'CC.C3_CIVILE')].parent" includes "CC"
jsonpath "$.facets.seat[?(@.value == 'CC.C3_CIVILE')].label" includes "3e chambre civile"


# ── 39. The seat filters on the path, and the tree stays in the filter ───────
GET {{base}}/internal/search?q=bail&seat=CC.C3_CIVILE
HTTP 200
[Asserts]
jsonpath "$.hits[0].seatPath.key" == "CC.C3_CIVILE"

GET {{base}}/internal/search/decompte?q=bail&seat=CC.C3_CIVILE
HTTP 200
[Asserts]
jsonpath "$.facets.seat[?(@.value == 'CC.C3_CIVILE')]" count == 1
jsonpath "$.facets.seat[?(@.value == 'CC.C2_CIVILE')]" count == 0


# ── 40. A room with no number is named by its specialisation ─────────────────
GET {{base}}/internal/search?q=licenciement&seat=CC.SOCIALE
HTTP 200
[Asserts]
jsonpath "$.hits[0].seatPath.key" == "CC.SOCIALE"

GET {{base}}/internal/search/decompte?q=licenciement&seat=CC.SOCIALE
HTTP 200
[Asserts]
jsonpath "$.facets.seat[?(@.value == 'CC.SOCIALE')]" count == 1
jsonpath "$.facets.seat[?(@.value == 'CC.SOCIALE')].parent" includes "CC"
jsonpath "$.facets.seat[?(@.value == 'CC.SOCIALE')].label" includes "Chambre sociale"


# ── 40b. A path the corpus does not carry is refused, and names the filter ───
GET {{base}}/internal/search?q=bail&seat=CC.CHAMBRE_DES_REVES
HTTP *
[Asserts]
status >= 400
status < 500


# ── 41. A bare number no body writes: the designated decision is the list ────
GET {{base}}/internal/search?q=362324
HTTP 200
[Asserts]
jsonpath "$.queryMode" == "reference"
jsonpath "$.hits" count == 1
jsonpath "$.hits[0].titleHtml" contains "362324"
jsonpath "$.pinned" not exists


# ── 41b. A bare run of six digits or more is the request, not a clue in one ──
GET {{base}}/internal/search?q=2503146
HTTP 200
[Asserts]
jsonpath "$.queryMode" == "reference"
jsonpath "$.hits" count >= 10
jsonpath "$.hits[0].titleHtml" contains "2503146"
jsonpath "$.pinned" not exists


# ── 41c. Five digits stay a postcode: a content list under a card ────────────
GET {{base}}/internal/search?q=75013
HTTP 200
[Asserts]
jsonpath "$.queryMode" == "lexical"
jsonpath "$.pinned" count >= 1


# ── 41d. The stated mode serves the citing decisions the bare run gave up ────
GET {{base}}/internal/search?q=2503146&mode=lexical
HTTP 200
[Asserts]
jsonpath "$.queryMode" == "lexical"
jsonpath "$.hits" count >= 1
jsonpath "$.pinned" not exists


# ── 42. A case name beside its court is a declared reference ─────────────────
GET {{base}}/internal/search?q=B%C3%A9ziers%20conseil%20d%27%C3%89tat
HTTP 200
[Asserts]
jsonpath "$.queryMode" == "reference"
jsonpath "$.hits" count >= 3
jsonpath "$.hits[0].titleHtml" contains "Béziers"
jsonpath "$.pinned" not exists
[Captures]
servies: jsonpath "$.allHitPaths" count


# ── 42b. Reference counts its own list: its question bounds nothing ──────────
GET {{base}}/internal/search/decompte?q=B%C3%A9ziers%20conseil%20d%27%C3%89tat
HTTP 200
[Asserts]
jsonpath "$.total" == {{servies}}


# ── 43. A court and a full day, no name, no number: a reference ──────────────
GET {{base}}/internal/search?q=caa%20paris%2017%20juillet%202024
HTTP 200
[Asserts]
jsonpath "$.queryMode" == "reference"
jsonpath "$.hits" count >= 10
jsonpath "$.hits[0].titleHtml" contains "Cour administrative d'appel de"
jsonpath "$.hits[0].titleHtml" contains "PA"
jsonpath "$.pinned" not exists


# ── 44. The QPC graphy is a docket key of the Conseil constitutionnel ────────
GET {{base}}/internal/search?q=2010-104%20QPC
HTTP 200
[Asserts]
jsonpath "$.queryMode" == "reference"
jsonpath "$.hits" count == 1
jsonpath "$.hits[0].titleHtml" contains "Conseil constitutionnel"
jsonpath "$.pinned" not exists


# ── 45. An ECLI is a key of its own ──────────────────────────────────────────
GET {{base}}/internal/search?q=ECLI:FR:CCASS:2007:C200527
HTTP 200
[Asserts]
jsonpath "$.queryMode" == "reference"
jsonpath "$.hits" count == 1
jsonpath "$.hits[0].titleHtml" contains "06-12.066"
jsonpath "$.pinned" not exists


# ── 46. A named article is counted by name, whatever its rank in the facet ───
GET {{base}}/internal/search/instruments?q=&needle=L761-2%20du%20code%20de%20la%20consommation
HTTP 200
[Captures]
citantes: jsonpath "$.legalInstrument[0].articles[0].count"
[Asserts]
jsonpath "$.legalInstrument[0].label" == "Code de la consommation"
jsonpath "$.legalInstrument[0].value" == "LEGITEXT000006069565"
jsonpath "$.legalInstrument[0].articles[0].label" == "L. 761-2"
jsonpath "$.legalInstrument[0].articles[0].value" == "l761-2"
jsonpath "$.legalInstrument[0].articles[0].count" > 0


# ── 46b. And that count is the total the composite filter serves ─────────────
GET {{base}}/internal/search/decompte?q=&legalArticle=LEGITEXT000006069565%7Cl761-2
HTTP 200
[Asserts]
jsonpath "$.total" == {{citantes}}


# ── 47. A bare number is an article here, and usage decides which code ───────
GET {{base}}/internal/search/instruments?q=&needle=700
HTTP 200
[Asserts]
jsonpath "$.legalInstrument[0].label" == "Code de procédure civile"
jsonpath "$.legalInstrument[0].articles[0].label" == "700"


# ── 48. The rail counts articles, the procedure bank out of that view alone ──
GET {{base}}/internal/search/decompte?q=clause%20abusive&limit=10
HTTP 200
[Asserts]
jsonpath "$.facets.legalArticleTop" count >= 5
jsonpath "$.facets.legalArticleTop[0].value" matches /\|/
jsonpath "$.facets.legalArticleTop[0].label" matches / · /
jsonpath "$.facets.legalArticleTop[*].label" not contains "700 · Code de procédure civile"
jsonpath "$.facets.legalInstrument[0].articles[*].value" contains "700"


# ── 48b. In browse mode, the composite filter serves what the facet counts ──
# Unfiltered, the facet count is read from the nightly snapshot; the filtered
# total is counted live. The snapshot trails the fund and never leads it, so the
# filter serves at least what the facet advertises. Measured during an ingest,
# the gap is what has been written since the last recount: 0.2 % on the most
# cited article of the civil code.
GET {{base}}/internal/search/decompte?q=&limit=10
HTTP 200
[Captures]
cite: jsonpath "$.facets.legalArticleTop[?(@.value=='LEGITEXT000006070721|1240')].count" nth 0

GET {{base}}/internal/search/decompte?q=&legalArticle=LEGITEXT000006070721%7C1240
HTTP 200
[Asserts]
jsonpath "$.total" >= {{cite}}



# ============================================================================
# AUTHED VARIANT (runner injects {{token}})
# The search endpoint is auth-OPTIONAL, so there is NO 401 contract: an
# unauthenticated call returns 200 with results, byte-identical with a token.
# ============================================================================
