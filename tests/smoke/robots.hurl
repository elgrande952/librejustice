# Smoke test — robots.txt rendu par la route
# Usage : hurl --variable api_url=http://localhost:8300 tests/smoke/robots.hurl --test
#
# Le fichier statique a été supprimé du bundle : si la route saute, la prod
# répond 404 et toute la politique d'accès disparaît d'un coup, sans autre
# signal. D'où ce contrat.

GET {{api_url}}/robots.txt
HTTP 200
[Asserts]
header "Content-Type" contains "text/plain"
# Le pré-entraînement reste refusé sur tout le site.
body contains "User-agent: ClaudeBot"
body contains "User-agent: GPTBot"
# Les indexeurs de citation sont autorisés, hors API.
body contains "User-agent: Claude-SearchBot"
body contains "User-agent: PerplexityBot"
body contains "Disallow: /api/"
# Le signal d'intention opposable et le sitemap sont servis.
body contains "Content-Signal: search=yes, ai-input=yes, ai-train=no"
body matches "(?m)^Sitemap: https://.+/sitemap\\.xml$"
