# ── security headers contract ────────────────────────────────────────────────
# Source oracle: apps/lj-server/src/main.rs (async fn security_headers)
#
# Ces en-têtes sont posés par un middleware que rien ne teste par ailleurs, et
# leur disparition est muette : le site répond 200 à l'identique. `Strict-
# Transport-Security` était conditionné à `settings.tls_cert_path.is_some()` —
# sortir le TLS du binaire l'aurait éteint sans un seul signal. D'où
# ce contrat, joué contre la cible live après tout deploy.

GET {{base}}/api/health
HTTP 200
[Asserts]
# HSTS un an, sans includeSubDomains ni preload (irrévocables, cf. doc du
# middleware). Posé sans condition côté origine ; le navigateur ne le voit
# jamais qu'en HTTPS.
header "strict-transport-security" contains "max-age=31536000"
header "x-content-type-options" == "nosniff"
header "x-frame-options" == "DENY"
header "referrer-policy" == "strict-origin-when-cross-origin"
header "permissions-policy" contains "geolocation=()"
# Socle CSP des réponses sans shell SSR (API, /pkg, /fonts, exports).
header "content-security-policy" contains "frame-ancestors 'none'"

# Même socle sur une page HTML, où la CSP complète du document est posée par le
# shell SSR avec son nonce : c'est ELLE qui doit répondre ici, pas le socle.
GET {{base}}/
HTTP 200
[Asserts]
header "strict-transport-security" contains "max-age=31536000"
header "x-content-type-options" == "nosniff"
header "content-security-policy" contains "nonce-"
