# Chemin AUTHENTIFIÉ de bout en bout — web (JWT Supabase) puis MCP (OAuth).
#
# Ce fichier est le SEUL contrôle automatique qui présente un jeton VALIDE. Le
# reste de la suite est volontairement sans secret et ne peut donc tester que le
# refus : `me.hurl` § bogus bearer attend un 401, et il reste vert même quand le
# serveur refuse TOUS les jetons valides. C'est ce trou qui a laissé la panne de
# vérification JWT du 07/08 vivre 2 j 23 h sans un symptôme visible.
#
# Ne se lance pas seul : `mise run contract-authed` provisionne le compte
# jetable, injecte `base` + `token`, et supprime le compte même en cas d'échec.
#
# PKCE : le couple vérifieur/défi est celui de l'annexe B de la RFC 7636, donc
# une constante — un client n'a pas à calculer un SHA-256 pour prouver que le
# serveur vérifie le sien. Même paire que le test unitaire
# `pkce_s256_challenge_matches_rfc7636_example` de `oauth.rs`.

# ── 1. Le profil se résout : c'est la vérification JWT elle-même ──────────────
GET {{base}}/api/me
Authorization: Bearer {{token}}
HTTP 200
[Asserts]
header "Content-Type" contains "application/json"
jsonpath "$.sub" isString
# L'identité résolue est CELLE du jeton, pas n'importe laquelle : c'est ce qui
# distingue « la vérification marche » de « le serveur rend un profil ».
# `displayName` n'est pas asserté — il est nul sur un compte neuf.
jsonpath "$.email" == "{{email}}"
jsonpath "$.createdAt" isString
jsonpath "$.lastSeenAt" isString
jsonpath "$.trackActivity" isBoolean

# ── 2. Une recherche identifiée aboutit ──────────────────────────────────────
# Le 401 n'est pas le seul symptôme possible : une identité perdue dégrade en
# anonyme sans erreur. On assert donc le succès, et `identity` se lit
# dans les spans.
GET {{base}}/api/search?q=trouble+anormal+de+voisinage&limit=3
Authorization: Bearer {{token}}
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection
jsonpath "$.total" isInteger

# ── 3. Enregistrement dynamique du client (RFC 7591) ─────────────────────────
POST {{base}}/oauth/register
Content-Type: application/json
{
  "client_name": "contract-probe",
  "redirect_uris": ["https://claude.ai/api/mcp/auth_callback"]
}
HTTP 201
[Captures]
client_id: jsonpath "$.client_id"

# ── 4. Approbation : c'est l'étape que la panne du 07/08 cassait ─────────────
# Elle exige un JWT Supabase valide. Un 401 ici = plus personne ne peut installer
# un connecteur, et c'est exactement ce qui s'est produit pendant 2 j 23 h.
POST {{base}}/oauth/approve
Authorization: Bearer {{token}}
Content-Type: application/json
{
  "client_id": "{{client_id}}",
  "redirect_uri": "https://claude.ai/api/mcp/auth_callback",
  "code_challenge": "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
}
HTTP 200
[Captures]
code: jsonpath "$.code"
[Asserts]
jsonpath "$.redirect_uri" == "https://claude.ai/api/mcp/auth_callback"

# ── 5. Échange du code contre une paire, PKCE vérifié ────────────────────────
POST {{base}}/oauth/token
[FormParams]
grant_type: authorization_code
code: {{code}}
client_id: {{client_id}}
redirect_uri: https://claude.ai/api/mcp/auth_callback
code_verifier: dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk
HTTP 200
[Captures]
access: jsonpath "$.access_token"
refresh: jsonpath "$.refresh_token"
[Asserts]
# Minuscule : c'est ce que le serveur rend, et la RFC 6749 §7.1 rend le champ
# insensible à la casse.
jsonpath "$.token_type" == "bearer"
jsonpath "$.expires_in" isInteger
jsonpath "$.refresh_token" isString

# ── 6. Le jeton MCP ouvre un appel d'outil réel ──────────────────────────────
POST {{base}}/mcp
Authorization: Bearer {{access}}
Accept: application/json, text/event-stream
Content-Type: application/json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "tools/call",
  "params": {
    "name": "search_decisions",
    "arguments": { "query": "trouble anormal de voisinage", "limit": 1 }
  }
}
HTTP 200
[Asserts]
jsonpath "$.jsonrpc" == "2.0"
jsonpath "$.result.isError" not == true

# ── 7. Le grant refresh tourne, et rote les deux jetons ──────────────────────
# Ce chemin n'avait jamais tourné en réel avant la sonde manuelle du 10/08 :
# 85 des 90 access tokens vivants dépendent de lui pour survivre à 30 jours.
POST {{base}}/oauth/token
[FormParams]
grant_type: refresh_token
refresh_token: {{refresh}}
client_id: {{client_id}}
HTTP 200
[Captures]
access2: jsonpath "$.access_token"
refresh2: jsonpath "$.refresh_token"
[Asserts]
jsonpath "$.access_token" != "{{access}}"
jsonpath "$.refresh_token" != "{{refresh}}"

# ── 8. Le jeton issu du refresh appelle un outil ─────────────────────────────
POST {{base}}/mcp
Authorization: Bearer {{access2}}
Accept: application/json, text/event-stream
Content-Type: application/json
{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "tools/call",
  "params": {
    "name": "search_decisions",
    "arguments": { "query": "servitude de passage", "limit": 1 }
  }
}
HTTP 200
[Asserts]
jsonpath "$.result.isError" not == true

# ── 9. L'ancien refresh est mort : la rotation tient ─────────────────────────
POST {{base}}/oauth/token
[FormParams]
grant_type: refresh_token
refresh_token: {{refresh}}
client_id: {{client_id}}
# Statut seul, comme le fait déjà `oauth.hurl` § 6d : la propriété à verrouiller
# est « l'ancien refresh ne marche plus », et le 400 l'exprime entièrement.
HTTP 400

# ── 10. Un jeton MCP inventé est refusé ──────────────────────────────────────
# Vérifie le motif d'alerte `mcp_token_rejete` : c'est la seule trace qu'un
# connecteur cassé laisse désormais côté serveur.
POST {{base}}/mcp
Authorization: Bearer lj_jeton_qui_nexiste_pas
Accept: application/json, text/event-stream
Content-Type: application/json
{ "jsonrpc": "2.0", "id": 3, "method": "tools/list", "params": {} }
HTTP 401
[Asserts]
jsonpath "$.error" == "invalid_token"
header "WWW-Authenticate" contains "resource_metadata="
