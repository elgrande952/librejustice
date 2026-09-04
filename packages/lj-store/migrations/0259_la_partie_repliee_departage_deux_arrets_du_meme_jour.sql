-- Migration 0259. La clé de partie, repliée d'un cran de plus.
--
-- Une décision de cassation nomme l'arrêt attaqué par la cour et la date,
-- jamais par le RG : mesuré le 25/08, le RG figure dans 101 textes sur 3 000.
-- Or cour et date ne désignent qu'un arrêt sur 42. Ce sont les parties qui
-- départagent, et `decision_party.resolve_key` ne suffit pas : il ôte la forme
-- sociale et la casse mais garde accents et ponctuation, là où un arrêt de CA
-- écrit « MUTUELLE DES ARCHITECTES FRANCAIS » pour « Mutuelle des architectes
-- français ». Ce cran de pli fait passer le rapprochement de 314 à 1 100
-- arrêts justes sur 3 000, pour 24 faux, mesuré contre le `contested` du greffe.

CREATE OR REPLACE FUNCTION lj_party_fold(x text) RETURNS text
LANGUAGE sql IMMUTABLE PARALLEL SAFE STRICT AS $$
  SELECT regexp_replace(
           translate(lower(x),
                     'àáâãäåçèéêëìíîïñòóôõöøùúûüýÿ',
                     'aaaaaaceeeeiiiinoooooouuuuyy'),
           '[^a-z0-9]', '', 'g')
$$;
