-- La colonne porte le genre : le contrat public dit `acteur`
-- (AnnonceHit) depuis sa création, seule la colonne parlait encore l'espèce
-- marchés. Et le contract du flux pur (0184) : closed_at n'est
-- plus ni lu ni écrit depuis la 0184, last_seen_at plus écrit depuis le
-- cycle précédent (write-once, un canal est une présence).
--
-- Fenêtre : l'app du cycle précédent lit acheteur_libelle et
-- last_seen_at. Déployer ce cycle SEUL, 0184-0190 déjà passées — le batch se
-- réduit alors à ces trois DDL de catalogue, la fenêtre à des millisecondes.

ALTER TABLE annonce RENAME COLUMN acheteur_libelle TO acteur_libelle;
ALTER TABLE annonce DROP COLUMN closed_at;
ALTER TABLE annonce_source DROP COLUMN last_seen_at;
