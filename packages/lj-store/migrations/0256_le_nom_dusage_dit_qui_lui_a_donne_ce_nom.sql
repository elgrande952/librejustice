-- Migration 0256. Le nom d'usage dit qui le lui a donné.
--
-- Quatre écrivains, que `case_name_source` sépare et ordonne. Chacun ne remplace
-- que ce qui lui est inférieur, et n'efface que le sien :
--
--   canon    écrit à la main contre l'usage, faute de source (`case_names.json`)
--   ce-list  les grandes décisions que le Conseil d'État publie lui-même
--   source   la source de la décision porte son nom (docname CEDH, titre CJUE)
--   renvoi   élu par les décisions qui la citent
--
-- L'ordre suit l'autorité, pas le volume : le nom que la CEDH donne à son propre
-- arrêt est plus sûr que celui qu'une citante lui donne. `lj_case_name_rank`
-- porte ce barème, que la pose relit à chaque écriture.

ALTER TABLE decisions
    ADD COLUMN case_name_source text
        CHECK (case_name_source IN ('canon', 'ce-list', 'source', 'renvoi'));

CREATE FUNCTION lj_case_name_rank(src text) RETURNS int
LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
    SELECT CASE src
        WHEN 'canon' THEN 3
        WHEN 'ce-list' THEN 2
        WHEN 'source' THEN 1
        WHEN 'renvoi' THEN 0
        ELSE -1
    END
$$;
