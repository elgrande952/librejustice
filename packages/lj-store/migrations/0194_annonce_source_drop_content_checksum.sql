-- Le DROP de content_checksum avait été ajouté à la 0184 après que son numéro
-- fut appliqué en prod (06/08 11:58) — un numéro enregistré ne se rejoue
-- jamais, le fichier 0184 est revenu à sa forme appliquée et le
-- DROP retombe ici. Sans lui, l'upsert write-once, qui n'écrit
-- plus la colonne (NOT NULL), échoue au premier canal nouveau.

ALTER TABLE annonce_source DROP COLUMN content_checksum;
