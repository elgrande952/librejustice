-- Migration 0278. La vue transitoire de la 0275 tombe : le serveur qui lisait
-- `decision_facet_counts` est remplacé depuis le déploiement du 29/08 à 01:30,
-- et le seul lecteur de la table est `corpus_counts` sous son nom.

DROP VIEW decision_facet_counts;
