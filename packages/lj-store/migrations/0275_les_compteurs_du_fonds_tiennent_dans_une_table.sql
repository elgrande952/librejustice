-- Migration 0275. `decision_facet_counts` devient `corpus_counts` et porte le
-- corpus entier, décisions et textes : l'accueil comptait les siens à la
-- lecture, 8,2 s pour le seul volet des articles en vigueur.
--
-- La vue porte le nom ancien le temps d'un déploiement : le serveur en place
-- lit encore la table, et la beta migre avant la bascule.
--
-- Les deux dimensions neuves sont semées ici, comme la 0231 avait semé les
-- siennes : le premier `precompute` est postérieur au déploiement, et un
-- accueil qui affiche zéro texte d'ici là serait faux, pas vide.

-- Semé avant le renommage : le comptage tient 8,2 s, le renommage prend un
-- verrou exclusif, et le serveur en place lit la table pendant ce temps-là.

INSERT INTO decision_facet_counts (dimension, value, parent, n)
    SELECT 'legal_text_total'::text, ''::text, NULL::text, count(*) FROM legal_text
UNION ALL
    SELECT 'legal_article_total', '', NULL, count(*) FROM (
        SELECT a.text_uid, a.num_key FROM legal_article a
         WHERE a.date_debut <= CURRENT_DATE
           AND (a.date_fin IS NULL OR a.date_fin >= CURRENT_DATE)
           AND a.kind = 'article'
         GROUP BY 1, 2) ids;

ALTER TABLE decision_facet_counts RENAME TO corpus_counts;
ALTER TABLE corpus_counts RENAME CONSTRAINT decision_facet_counts_key TO corpus_counts_key;
CREATE VIEW decision_facet_counts AS SELECT * FROM corpus_counts;
