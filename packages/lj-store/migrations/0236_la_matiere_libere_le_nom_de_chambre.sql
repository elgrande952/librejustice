-- Migration 0236 : la spécialisation devient la matière, `chamber` libère son
-- nom. `chamber:*` classe le contentieux (CIVILE, ETRANGERS…), pas la chambre.
--
-- Additive : l'ancien serveur lit encore `chamber_uid` et la facette `chamber`
-- pendant `migrate`. Les deux vocabulaires cohabitent jusqu'au déploiement
-- suivant, qui droppe le premier.

ALTER TABLE decisions
  ADD COLUMN matiere_uid TEXT REFERENCES facet_value(uid);

INSERT INTO facet_value (uid, facet, label, abbr, parent_uid, sort, hint)
SELECT 'matiere:' || split_part(uid, ':', 2), 'matiere', label, abbr, NULL, sort, hint
  FROM facet_value
 WHERE facet = 'chamber';
