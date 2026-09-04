-- Une fiche DCE porte une décision, un bundle de commentaires n'en porte pas :
-- partager la source `ariane-web` leur donnait le même rang, et le départage
-- alphabétique servait le bundle à la ré-extraction de 93 décisions du Conseil
-- d'État. Colonne GÉNÉRÉE, donc DROP puis ADD : la table est réécrite.
ALTER TABLE decision_sources DROP COLUMN source_rank;

ALTER TABLE decision_sources
    ADD COLUMN source_rank SMALLINT NOT NULL GENERATED ALWAYS AS (
        CASE source
            WHEN 'judilibre'    THEN 60
            WHEN 'opendata'     THEN 55
            WHEN 'dila-jade'    THEN 50
            WHEN 'dila-constit' THEN 50
            WHEN 'cedh'         THEN 50
            WHEN 'cjue'         THEN 50
            WHEN 'cnda'         THEN 50
            WHEN 'ariane-dce'   THEN 45
            WHEN 'dila-cass'    THEN 30
            WHEN 'dila-capp'    THEN 30
            ELSE 0
        END
    ) STORED;

-- L'assemblée plénière de la Cour de cassation et la formation plénière d'une
-- de ses chambres sont deux formations, longtemps confondues sous un seul uid.
INSERT INTO facet_value (uid, facet, label, sort) VALUES
    ('formation:ASSEMBLEE_PLENIERE', 'formation', 'Assemblée plénière', 13);
