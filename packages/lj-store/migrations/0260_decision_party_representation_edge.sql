ALTER TABLE decision_party
  ADD COLUMN IF NOT EXISTS party_ord integer,
  ADD COLUMN IF NOT EXISTS sub_ord integer;

CREATE INDEX IF NOT EXISTS decision_party_party_ord_idx
  ON decision_party (decision_id, party_ord)
  WHERE party_ord IS NOT NULL;

CREATE INDEX IF NOT EXISTS decision_party_fold_morale_idx
  ON decision_party (lj_party_fold(resolve_key))
  WHERE quality = 'party' AND nature IS DISTINCT FROM 'physique';
