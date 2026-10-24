# LibreJustice : API autonome de recherche lexicale

Ce dossier prépare une **nouvelle API autonome**, avec le moteur et les tables LibreJustice épinglés au commit `ca1d06062ec4bb2cb332b0e9d41a45309fa3ad75`. Il ne remplace aucun service existant. Un corpus initial de **10 décisions officielles de la chambre sociale** a été lu intégralement via l'API officielle Judilibre le 24 octobre 2026. Les originaux JSON, dates, identifiants, empreintes de réponse et Licence Ouverte 2.0 sont conservés dans `corpus/`.

**Ce n'est pas le corpus complet LibreJustice.** Aucun texte normatif n'est importé par ce premier lot. Les 10 décisions servent à une première recherche juridique réelle, pas à prouver l'exhaustivité ni la pertinence d'une analyse juridique. Leurs dates vont de 2011 à 2025; les requêtes ont porté sur l'obligation de sécurité, le harcèlement moral et les heures supplémentaires.

## Différences vérifiables avec le test précédent

- Le runtime de production ne crée jamais `DummyEmbedder`. Un nouveau backend `Disabled` refuse explicitement toute demande de vecteurs, sans appel réseau ni téléchargement.
- Le serveur expose uniquement `/health`, `/search`, `/decision/{id}` et `/corpus`. Les routes d'administration, OAuth, courriels, IA, PDF, MCP amont et écritures utilisateur ne sont pas montées.
- Les trois routes de données demandent un Bearer secret de 32 à 256 caractères; la comparaison est vérifiée par HMAC. Le secret est fourni par l'hébergeur et n'est jamais écrit dans ce dépôt ou dans les journaux.
- La recherche impose `lexical`, refuse `semantic`, `auto`, `reference` et `aiMode=true`. Elle n'utilise ni modèle ni fournisseur LLM. La pagination est bornée et le pool à 8 connexions.
- L'import utilise `parse_judilibre`, `build_source_fields` et `DecisionRepository::upsert` amont. Il n'exécute pas `lj-ingest` (dont la compilation télécharge un tokenizer). Aucune décision synthétique n'est créée. Aucune section ni métadonnée juridique n'est inventée.
- Les champs structurés minimaux (date, numéros, titre composé par le moteur amont, publication, thèmes) sont persistés explicitement depuis la source. Les classifications enrichies, citations extraites et liens de parties ne sont pas calculés par cet import minimal. L'empreinte d'idempotence inclut la version de cet import; les empreintes canoniques de réponse officielle restent séparées dans le manifeste de provenance.
- Chaque ligne JSONL est validée avant la première écriture. L'URL officielle, l'identifiant, la licence, les dates et le texte complet sont requis. Chaque décision est transactionnelle et son empreinte rend la réimportation idempotente; le fichier original n'est pas modifié. Aucune suppression n'est effectuée.
- Les trois seuls fichiers amont changés sont gardés par SHA-256 dans `runtime/patch-manifest.json` : backend/refus, sélection du backend, et le correctif 0204 déjà testé pour base vide.

## Installation sur l'abonnement existant

Deux services dédiés sont nécessaires; aucun volume ou base existant ne doit être réaffecté.

1. **PostgreSQL** : construire `Dockerfile.pg` depuis ce dossier. C'est le même assemblage testé : ParadeDB `0.25.6-pg18`, VectorChord `1.1.1`. Définir `POSTGRES_USER=librejustice`, `POSTGRES_DB=librejustice`, un `POSTGRES_PASSWORD` secret et `PGDATA=/var/lib/postgresql/data/pgdata`. Monter un nouveau volume dédié sur `/var/lib/postgresql/data`. Le script passe par l'entrypoint officiel PostgreSQL pour initialiser les droits du volume, puis le processus PostgreSQL s'exécute en utilisateur postgres. Garder la base sur le réseau privé, sans proxy public.
2. **API** : construire `Dockerfile.api`. Définir `LIBREJUSTICE_DB_URL` vers cette base privée (avec mot de passe par référence de variable), `LJ_ACCESS_TOKEN` secret et `LJ_ALLOW_SCHEMA_MIGRATIONS=1`. La commande `boot` applique les migrations puis importe le lot idempotent avant d'écouter; `/health` vérifie réellement la base. Le port suit `PORT` de l'hébergeur, sinon 8080. Une seule réplique. Activer HTTPS via le domaine de service.

Les migrations ne sont autorisées que sur cette base dédiée. Après la première mise en service, `serve` démarre en vérifiant seulement le schéma; `import chemin.jsonl` ajoute un lot autorisé séparément. Le corpus intégré ne vaut pas synchronisation régulière : les mises à jour, retraits/pseudonymisations et l'extension à d'autres fonds restent à organiser avant une diffusion publique exhaustive.

Le test précédent de base vide mesurait 244,6 MiB pour PostgreSQL, sous une limite de 2 GiB. Ce n'est **pas** une mesure du besoin en production ni une promesse de coût nul. Les limites CPU/mémoire et le stockage doivent être réglés par l'opérateur selon les ressources existantes; ne pas créer un abonnement ou acheter des crédits. Une sauvegarde durable du nouveau volume est nécessaire; le Git contient le code et le lot initial, pas les futures données de la base.

## Contrat de raccordement

`GET /health` est public, sans contenu juridique ni secret. Les autres appels utilisent `Authorization: Bearer <LJ_ACCESS_TOKEN>`.

```text
GET /search?q=obligation%20de%20s%C3%A9curit%C3%A9&limit=10&offset=0
GET /decision/{id}
GET /corpus
```

`/search` rend `SearchResponse` amont (`hits[].id`, `officialUrl`, `queryMode`, `reranked`); `/decision` rend `DecisionDetail` (`id`, `paragraphs`, métadonnées). `/corpus` rend les nombres réellement présents et `complete:false`. Aucun raccordement ChatGPT n'est créé par ce fichier; la façade MCP doit être branchée à l'URL HTTPS et à son secret par le propriétaire du déploiement.

## Validation

Le workflow proposé est `librejustice-lexical-production.yml`, à installer sur la branche par défaut du fork public; copier ce dossier sous `sol-fable-production/` et passer le commit exact dans `kit_sha`. Il compile le nouveau code, teste la politique lexicale et le refus du backend, lance une base isolée, importe les 10 décisions réelles, vérifie recherche/lecture/authentification/refus IA, puis vérifie qu'une réimportation ne crée aucun doublon et qu'aucun chunk vectoriel n'existe. Il supprime ensuite les conteneurs et volumes temporaires. Il ne publie ni image ni artefact, et ne reçoit aucune clé fournisseur.

À ce stade, les contrôles statiques locaux sont disponibles. **La compilation et les tests de ce nouveau runtime doivent être confirmés par le résultat CI; les anciens 338 tests de migration ne prouvent pas ce nouveau binaire.** Les preuves CI et de déploiement sont à ajouter après exécution réelle.
