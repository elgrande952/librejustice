ALTER TABLE decisions
  ADD COLUMN matiere_uid TEXT REFERENCES facet_value(uid);

INSERT INTO facet_value (uid, facet, label, abbr, parent_uid, sort, hint)
SELECT 'matiere:' || split_part(uid, ':', 2), 'matiere', label, abbr, NULL, sort, hint
  FROM facet_value
 WHERE facet = 'chamber';
