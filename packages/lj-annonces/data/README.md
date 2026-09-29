# Données embarquées `lj-annonces`

Embarquées via `include_str!`, pures : aucune I/O au runtime.

## `zones_fr.tsv` : départements, NUTS3 et régions

Une ligne par département (101) et par collectivité d'outre-mer (8), colonnes `departement TAB nuts3 TAB region`. Consommé par `src/zones.rs` : traduit en département le code NUTS3 que TED sert sur 73 % de ses avis FR ouverts, éclate une région en ses départements, et rend la région d'un département pour grouper la facette géographique.

- **Source** : classification NUTS 2024 d'Eurostat, niveaux 1 et 3, servie par [Nuts2json](https://github.com/eurostat/Nuts2json).
- **Licence** : réutilisation autorisée avec mention de la source (politique de réutilisation Eurostat).
- **Construction** : chaque NUTS3 est apparié par son nom au référentiel `lj_norm::referential_labels::departement_label` (96 sur 96, sans reste) ; la région est le NUTS1, qui porte les 13 régions métropolitaines actuelles là où le NUTS2 sert encore celles d'avant 2016. Les cinq DROM (`FRY10` à `FRY50`) et les huit COM, absents du fichier de géométries, sont ajoutés. Contrôle croisé sur les 395 avis TED mono-lot portant à la fois un NUTS3 et un code postal : 393 d'accord, 2 désaccords explicables (une boîte cedex, un lieu d'exécution distinct du siège de l'acheteur).
