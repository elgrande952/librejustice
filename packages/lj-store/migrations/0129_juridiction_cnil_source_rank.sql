INSERT INTO facet_value (uid, facet, label, abbr, sort) VALUES
    ('juridiction:CNIL', 'juridiction', 'Commission nationale de l''informatique et des libertés', 'CNIL', 15);

ALTER TABLE decision_sources DROP COLUMN source_rank;

ALTER TABLE decision_sources
    ADD COLUMN source_rank SMALLINT NOT NULL GENERATED ALWAYS AS (
        CASE source
            WHEN 'judilibre'    THEN 60
            WHEN 'opendata'     THEN 55
            WHEN 'dila-jade'    THEN 50
            WHEN 'dila-constit' THEN 50
            WHEN 'dila-cnil'    THEN 50
            WHEN 'cedh'         THEN 50
            WHEN 'cjue'         THEN 50
            WHEN 'cnda'         THEN 50
            WHEN 'dila-cass'    THEN 30
            WHEN 'dila-capp'    THEN 30
            ELSE 0
        END
    ) STORED;
