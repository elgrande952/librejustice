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
