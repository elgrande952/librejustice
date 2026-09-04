# Contrat des routes /internal/auth : les refus, sans secret ni compte créé.
#
# Les écrans de compte sont des formulaires sans JS : une erreur attendue répond
# 303 avec son code en query — jamais l'adresse, jamais un corps JSON. Le chemin
# qui réussit demande des identifiants valides, il vit dans `contract-authed`.

# ── Connexion refusée : rebond sur la page, `next` conservé ──────────────────
POST {{base}}/internal/auth/login
[FormParams]
email: probe@example.invalid
password: MotDePasse1
next: /decisions
HTTP 303
[Asserts]
header "Location" == "/login?error=auth_required&next=%2Fdecisions"

# ── Inscription : mot de passe trop faible, refusé avant toute écriture ──────
POST {{base}}/internal/auth/signup
[FormParams]
email: probe@example.invalid
password: faible
HTTP 303
[Asserts]
header "Location" == "/login?mode=signup&error=mot_de_passe_trop_faible"

# ── Inscription : adresse invalide ───────────────────────────────────────────
POST {{base}}/internal/auth/signup
[FormParams]
email: sans-arobase
password: MotDePasse1
HTTP 303
[Asserts]
header "Location" == "/login?mode=signup&error=email_invalide"

# ── Lien de mail inconnu : formulaire, l'erreur rebondit sur la page ─────────
POST {{base}}/internal/auth/reset
Content-Type: application/x-www-form-urlencoded
[FormParams]
token: jeton-qui-n-existe-pas
password: MotDePasse1
confirm: MotDePasse1
HTTP 303
[Asserts]
header "Location" == "/reset-password?error=lien_invalide_ou_expire"

# ── Mots de passe différents : rebond avec le jeton conservé ─────────────────
POST {{base}}/internal/auth/reset
Content-Type: application/x-www-form-urlencoded
[FormParams]
token: jeton-qui-n-existe-pas
password: MotDePasse1
confirm: MotDePasse2
HTTP 303
[Asserts]
header "Location" == "/reset-password?error=mots_de_passe_differents&token=jeton-qui-n-existe-pas"

# ── Confirmation : jeton mort, rebond avec son code ──────────────────────────
POST {{base}}/internal/auth/confirm
Content-Type: application/x-www-form-urlencoded
[FormParams]
token: jeton-qui-n-existe-pas
HTTP 303
[Asserts]
header "Location" == "/confirm?error=lien_invalide_ou_expire"

# ── Changement d'adresse : jeton mort, rebond avec son code ─────────────────
POST {{base}}/internal/auth/email-change
Content-Type: application/x-www-form-urlencoded
[FormParams]
token: jeton-qui-n-existe-pas
HTTP 303
[Asserts]
header "Location" == "/email-change?error=lien_invalide_ou_expire"

# ── Retours du tour Google, avant l'aller ────────────────────────────────────

# ── Retour sans cookie d'état : rien à recomparer ────────────────────────────
GET {{base}}/internal/auth/google/callback?code=code-invente&state=etat-invente
HTTP 400
[Asserts]
jsonpath "$.detail" == "etat_invalide"

# ── Retour dont l'état ne correspond pas au cookie ───────────────────────────
GET {{base}}/internal/auth/google/callback?code=code-invente&state=pas-le-bon
Cookie: lj_google_oauth=etat-de-test.Lw
HTTP 400
[Asserts]
jsonpath "$.detail" == "etat_invalide"

# ── Consentement refusé : Google renvoie l'état sans code ────────────────────
# `Lw` est `base64url("/")`, la destination rangée dans le cookie.
GET {{base}}/internal/auth/google/callback?state=etat-de-test
Cookie: lj_google_oauth=etat-de-test.Lw
HTTP 303
[Asserts]
header "Location" == "/"
cookie "lj_google_oauth[Max-Age]" == 0

# ── Troisième état à la porte : cookie présenté et refusé ────────────────────
# 401 `session_rejected`, distinct d'`auth_required`, plus l'effacement du
# cookie — sans quoi le navigateur le represente à chaque requête suivante.
GET {{base}}/internal/me
Cookie: __Host-lj_session=cookie-qui-n-existe-pas
HTTP 401
[Asserts]
jsonpath "$.detail" == "session_rejected"
cookie "__Host-lj_session[Max-Age]" == 0

# ── Visiteur anonyme : pas de cookie, pas de refus ───────────────────────────
GET {{base}}/internal/me
HTTP 401
[Asserts]
jsonpath "$.detail" == "auth_required"

# ── Tour Google, aller : le 303 part vers Google et l'état est posé ──────────
# En dernier : le bocal à cookies de hurl vaut pour tout le fichier, et le vrai
# `lj_google_oauth` posé ici évince l'en-tête `Cookie:` des requêtes suivantes.
GET {{base}}/internal/auth/google/start?next=%2Fdecisions
HTTP 303
[Asserts]
header "Location" startsWith "https://accounts.google.com/o/oauth2/v2/auth?client_id="
header "Location" contains "&response_type=code"
header "Location" contains "&scope=openid+email+profile"
header "Location" contains "&prompt=select_account"
header "Location" contains "&state="
header "Location" contains "%2Finternal%2Fauth%2Fgoogle%2Fcallback"
cookie "lj_google_oauth" exists
cookie "lj_google_oauth[HttpOnly]" exists
cookie "lj_google_oauth[Secure]" exists
cookie "lj_google_oauth[SameSite]" == "Lax"
cookie "lj_google_oauth[Max-Age]" == 600
