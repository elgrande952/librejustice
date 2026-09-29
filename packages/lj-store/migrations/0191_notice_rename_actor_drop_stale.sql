ALTER TABLE annonce RENAME COLUMN acheteur_libelle TO acteur_libelle;
ALTER TABLE annonce DROP COLUMN closed_at;
ALTER TABLE annonce_source DROP COLUMN last_seen_at;
