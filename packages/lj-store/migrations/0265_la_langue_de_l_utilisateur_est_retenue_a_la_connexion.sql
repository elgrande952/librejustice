-- Migration 0265. La langue de l'utilisateur est retenue à la connexion.
--
-- `browser_locale` vient de l'en-tête `Accept-Language`, `provider_locale` du
-- claim `locale` de l'id_token : la langue de lecture et celle de l'interface
-- du fournisseur ne sont pas la même. Chacune se pose à la première connexion
-- qui la porte et ne bouge plus.

ALTER TABLE users
    ADD COLUMN browser_locale  text,
    ADD COLUMN provider_locale text;
