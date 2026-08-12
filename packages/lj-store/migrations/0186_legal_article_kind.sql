-- Migration 0186 — Une ligne d'article dit ce qu'elle est.
--
-- Trois acteurs se croisent sur une modification : la parution au
-- JO qui porte le texte performatif, l'edit côté LEGI qui n'en garde qu'une
-- mention et porte l'arête, la version cible qui porte la fenêtre de validité.
-- Seul le dernier est une norme. `kind` nomme la distinction au lieu de la
-- laisser se déduire d'un artefact : la fenêtre de validité
-- excluait les edits parce que leur `date_debut` sentinelle n'est jamais
-- `<= aujourd'hui`, ce qui marche par accident et rate les 2 361 edits que
-- DILA date normalement (dont 1 647 en `VIGUEUR`, aujourd'hui dans la
-- recherche).
--
-- `NOT NULL DEFAULT` sans expression volatile : Postgres l'inscrit dans le
-- catalogue sans réécrire la table (métadonnée seule), donc sans verrou tenu.
-- La valeur réelle est posée par `refresh_article_denorm`, qui tourne en fin de
-- chaque ingest écrivant des articles — une migration porte du
-- schéma, la réécriture de masse est une commande `lj-ingest`.

ALTER TABLE legal_article
    ADD COLUMN kind text NOT NULL DEFAULT 'article';

COMMENT ON COLUMN legal_article.kind IS
  'Nature de la ligne : ''article'' (norme ou publication) | ''edit'' (dispositif consommé, LEGI n''en garde que la mention de sa cible). Posée par refresh_article_denorm depuis le corps.';
