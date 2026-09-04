-- Migration 0225 : compteurs de décisions par juridiction et par année.
--
-- Remplace trois agrégats pleine table dans le chemin de requête (catalogue
-- `/juridictions`, histogramme `/juridiction/{code}`, nombres de pages du
-- sitemap) : 3,73 M lignes et 9,2 Go de heap scannés par appel, 1,07 s à vide et
-- 13 à 15 s dès qu'un crawler en lance une trentaine de front, contre 2 491
-- lignes à lire ici, et 0,36 ms mesurés pour l'histogramme.
--
-- `year` est NULLABLE et porte les décisions sans `date_lecture` (8 sur 7 codes
-- au 21/08/2026) : le catalogue les compte, l'histogramme non, et seule une
-- décomposition qui préserve le total sert les deux sans déplacer un chiffre.
-- D'où l'unicité en NULLS NOT DISTINCT, qu'une clé primaire interdirait, et qui
-- donne à `ON CONFLICT` la ligne nulle comme les autres.
--
-- Écrivain unique : l'étape `resync-jurisdiction-counts`, quotidienne, avant
-- `sitemap`. La jointure sur `jurisdiction` reste côté lecteur.

CREATE TABLE jurisdiction_year_counts (
    jurisdiction_code text   NOT NULL,
    year              int,
    decision_count    bigint NOT NULL,
    CONSTRAINT jurisdiction_year_counts_key
        UNIQUE NULLS NOT DISTINCT (jurisdiction_code, year)
);

INSERT INTO jurisdiction_year_counts (jurisdiction_code, year, decision_count)
SELECT jurisdiction_code, extract(year FROM date_lecture)::int, count(*)
FROM decisions
WHERE deleted_at IS NULL
GROUP BY 1, 2;
