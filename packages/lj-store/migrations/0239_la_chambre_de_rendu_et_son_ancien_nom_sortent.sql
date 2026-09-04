-- Migration 0239 : second temps du passage à `chamber_key` / `matiere_uid`.
-- Plus aucun lecteur des deux colonnes : la ré-extraction v78 a rempli les
-- neuves, et le serveur debout pendant `migrate` ne lit déjà plus les vieilles.

ALTER TABLE decisions
  DROP COLUMN chamber_position,
  DROP COLUMN chamber_uid;

DELETE FROM facet_value WHERE facet = 'chamber';
