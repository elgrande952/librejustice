# ============================================================================
# Contract tests: route groups `me` (profile, bookmarks) and `account`
# (résumé, display-name, password-link, unlink, email-change, delete). All
# require auth (RequiredUser).
#
# These tests assert the UNAUTHENTICATED contract (401 shape) for every
# endpoint. The authenticated path lives in `authed.hurl`, which opens a real
# session.
#
# KNOWN PARITY RISK (see notes): the 401 BODY differs between oracles.
#   Python (FastAPI HTTPException) -> JSON  {"detail":"auth_required"}
#   Rust   (ApiError::Unauthorized) -> text/plain  "unauthorized"
# We assert ONLY status==401 here so the file is green against BOTH oracles;
# the body divergence is documented for the A/B differ to flag.
# ============================================================================

# ── GET /internal/me, unauth ────────────────────────────────────────────────
GET {{base}}/internal/me
HTTP 401

# ── GET /internal/me, un en-tête Authorization n'ouvre rien ─────────────────
# Les routes REST ne lisent QUE le cookie de session : un porteur, valide ou
# non, n'y est pas une identité. Le chemin authentifié vit dans `authed.hurl`.
GET {{base}}/internal/me
Authorization: Bearer not-a-real-jwt
HTTP 401

# ── GET /internal/account, unauth ────────────────────────────────────────────
GET {{base}}/internal/account
HTTP 401

# ── POST /internal/account/display-name, unauth (auth before body validation) ─
POST {{base}}/internal/account/display-name
[Form]
display_name: Jean Dupont
HTTP 401

# ── POST /internal/account/password-link, unauth ─────────────────────────────
POST {{base}}/internal/account/password-link
HTTP 401

# ── POST /internal/account/unlink, unauth ────────────────────────────────────
POST {{base}}/internal/account/unlink
[Form]
provider: google
provider_sub: x
HTTP 401

# ── POST /internal/account/email-change, unauth ──────────────────────────────
POST {{base}}/internal/account/email-change
[Form]
email: nouvelle@example.org
HTTP 401

# ── POST /internal/account/delete, unauth ────────────────────────────────────
POST {{base}}/internal/account/delete
[Form]
confirm: SUPPRIMER
HTTP 401

# ── GET /internal/me/bookmarks, unauth ──────────────────────────────────────
GET {{base}}/internal/me/bookmarks
HTTP 401

# ── PUT /internal/me/bookmarks/{id}, unauth ─────────────────────────────────
PUT {{base}}/internal/me/bookmarks/ABC123
HTTP 401

# ── DELETE /internal/me/bookmarks/{id}, unauth ──────────────────────────────
DELETE {{base}}/internal/me/bookmarks/ABC123
HTTP 401
