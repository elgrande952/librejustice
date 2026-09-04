# Contrat de /internal/signalement : les refus, et le leurre, jamais un envoi réel.
#
# Formulaire sans JS : 303 vers /feedback avec son code en query, la page
# d'origine conservée dans `de`. Aucun cas ici n'émet de courrier : les
# refus s'arrêtent avant, et le leurre rend le 303 du succès sans rien envoyer.
# C'est ce qui rend la suite rejouable contre la prod sans remplir la boîte.

# ── Leurre rempli : accepté en apparence, rien n'est émis ────────────────────
POST {{base}}/internal/signalement
[FormParams]
sujet: technique
url: /decisions
message: Un robot qui remplit tous les champs qu'il trouve, y compris celui-ci.
site: http://spam.invalid
HTTP 303
[Asserts]
header "Location" == "/feedback?sent=1&from=%2Fdecisions"

# ── Message trop court : refusé avant toute émission ─────────────────────────
POST {{base}}/internal/signalement
[FormParams]
sujet: manque
url: /decision/abc123
message: trop
HTTP 303
[Asserts]
header "Location" == "/feedback?error=message_trop_court&from=%2Fdecision%2Fabc123"

# ── Adresse illisible : refusée, un en-tête ne porte pas ça ──────────────────
POST {{base}}/internal/signalement
[FormParams]
sujet: autre
url: /
message: Le message est assez long pour passer le plancher de validation.
email: pas-une-adresse
HTTP 303
[Asserts]
header "Location" == "/feedback?error=adresse_invalide&from=%2F"

# ── Destination hors site : ramenée à la racine, pas de redirection ouverte ──
POST {{base}}/internal/signalement
[FormParams]
sujet: autre
url: https://evil.invalid/
message: trop
HTTP 303
[Asserts]
header "Location" == "/feedback?error=message_trop_court&from=%2F"

# ── Nature inconnue : formulaire fabriqué à la main, refusé par le type ──────
POST {{base}}/internal/signalement
[FormParams]
sujet: inconnu
url: /
message: Le message est assez long pour passer le plancher de validation.
HTTP 422
