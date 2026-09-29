ALTER TABLE users
    ADD COLUMN browser_languages text[],
    ADD COLUMN country text;

UPDATE users SET browser_languages = ARRAY[browser_locale] WHERE browser_locale IS NOT NULL;
