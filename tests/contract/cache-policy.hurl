# ── politique de cache du HTML ───────────────────────────────────────────────
# Source oracle : apps/lj-server/src/main.rs (html_cache_policy) + pages SSR
# (`ResponseOptions`). Le défaut est private, no-store ; une page publique
# déclare son cache elle-même. L'accident verrouillé ici est silencieux dans
# les deux sens : un défaut redevenu public servirait l'HTML d'un visiteur à un
# autre, une page document redevenue privée éteindrait le cache edge sans un
# signal.

# Écrans d'auth : le défaut privé, sans déclaration.
GET {{base}}/login
HTTP 200
[Asserts]
header "Cache-Control" == "private, no-store"

GET {{base}}/confirm
HTTP 200
[Asserts]
header "Cache-Control" == "private, no-store"

GET {{base}}/reset-password
HTTP 200
[Asserts]
header "Cache-Control" == "private, no-store"

# Le consentement MCP et le profil exigent une session avant le rendu.
GET {{base}}/authorize-mcp
HTTP 303
[Asserts]
header "Location" startsWith "/login?next="

GET {{base}}/profile
HTTP 303
[Asserts]
header "Location" == "/login?next=%2Fprofile"

# Pages publiques : cache déclaré, 5 min.
GET {{base}}/
HTTP 200
[Asserts]
header "Cache-Control" == "public, max-age=0, must-revalidate, s-maxage=300"

GET {{base}}/decision-search
HTTP 200
[Asserts]
header "Cache-Control" == "public, max-age=0, must-revalidate, s-maxage=300"

# Page document : publique dans tous les cas — 7 j de CDN, ou 5 min tant que le
# résumé manque (decision_page.rs), d'où l'assertion sur le préfixe.
GET {{base}}/internal/search?q=usufruit&limit=1
HTTP 200
[Captures]
decision_id: jsonpath "$.hits[0].id"

GET {{base}}/decision/{{decision_id}}
[Options]
location: true
HTTP 200
[Asserts]
header "Cache-Control" startsWith "public, max-age=0, s-maxage="
