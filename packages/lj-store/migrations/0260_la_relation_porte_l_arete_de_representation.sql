-- Migration 0260. La relation porte l'arête de représentation.
--
-- `party_ord` : l'`ord` de la ligne partie représentée. `sub_ord` : celui du
-- conseil substitué. Même patron que `firm_ord`, une référence de ligne
-- intra-décision sans FK, la table étant réécrite au grain décision.

ALTER TABLE decision_party
  ADD COLUMN IF NOT EXISTS party_ord integer,
  ADD COLUMN IF NOT EXISTS sub_ord integer;

CREATE INDEX IF NOT EXISTS decision_party_party_ord_idx
  ON decision_party (decision_id, party_ord)
  WHERE party_ord IS NOT NULL;

-- Le pseudonyme est réattribué par décision : « Vincent X... » en cassation et
-- « Vincent X... » en appel sont deux personnes différentes. Une partie
-- physique ne se rapproche donc jamais d'un degré à l'autre.

CREATE INDEX IF NOT EXISTS decision_party_fold_morale_idx
  ON decision_party (lj_party_fold(resolve_key))
  WHERE quality = 'party' AND nature IS DISTINCT FROM 'physique';
