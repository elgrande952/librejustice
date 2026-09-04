-- Migration 0240 : l'issue de la victime directe entre dans l'observation.
--
-- Le préjudice d'affection d'un proche relève de deux barèmes disjoints selon
-- que la victime directe est décédée ou survit, et le référentiel ne chiffre
-- que le premier. Une médiane qui les mélange ne décrit ni l'un ni l'autre.

ALTER TABLE quantum_observation ADD COLUMN deces boolean;
