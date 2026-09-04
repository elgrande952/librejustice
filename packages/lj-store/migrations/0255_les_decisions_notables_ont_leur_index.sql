-- Les notables sont 4,8 % du corpus ; sans cet index la jambe IDF des décisions
-- similaires relit `decisions` 270 967 fois par affichage (1 718 ms contre 607).
-- Le prédicat reprend la clause de la requête mot pour mot, `deleted_at` exclu :
-- un index partiel ne sert que si le planificateur prouve l'implication.
CREATE INDEX idx_decisions_notable ON decisions (id)
WHERE publication_codes && ARRAY['r', 'A', 'b', 'l', 'c', 'B', 'C+', 'R']::text[];
