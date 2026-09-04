-- Migration 0222 — `users.google_sub` disparaît : la 0217 avait repris ses
-- valeurs dans `user_identities` et l'avait laissée morte le temps d'un
-- déploiement.
--
-- Plus rien ne la lit : `google_auth.rs::rapprocher` interroge
-- `user_identities (provider, provider_sub)`, et les trois écritures du chemin
-- Google visent la même table. La colonne ne servait plus qu'à mentir : elle
-- portait 73 lignes contre 75 identités, écart qui grandissait d'une unité à
-- chaque inscription, et toute lecture « inscriptions Google par jour » posée
-- dessus sous-comptait sans le dire.
--
-- Compter par `user_identities.linked_at` ne rattrape pas l'histoire : la 0217
-- a horodaté les 75 lignes de sa propre reprise. La forme juste part de
-- `users.created_at` avec l'identité en test d'existence.
--
-- L'index unique partiel `users_google_sub_uidx` tombe avec la colonne.

ALTER TABLE users DROP COLUMN google_sub;
