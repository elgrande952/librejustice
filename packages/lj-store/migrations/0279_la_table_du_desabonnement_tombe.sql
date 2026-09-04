-- Migration 0279 : la table du désabonnement tombe. L'état ne vit plus du côté
-- du produit, qui n'a plus ni route ni page pour l'écrire, et sa ligne unique
-- est déjà recopiée là où elle est lue.

DROP TABLE comptes.unsubscribes;
