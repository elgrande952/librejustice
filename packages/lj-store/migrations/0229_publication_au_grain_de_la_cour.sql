-- La publication se filtre en appartenance multiple sur `publication_codes`,
-- déjà indexé en `pdb.literal` par `decisions_bm25` : rien à reconstruire.
-- Additif seulement : la beta applique les migrations en attente sur la base de
-- prod, où l'ancien serveur lit encore `publication_uid`. Sa suppression, et
-- celle des valeurs obsolètes, partent au déploiement d'après.

ALTER TABLE facet_value ADD COLUMN hint TEXT;

INSERT INTO facet_value (uid, facet, label, sort) VALUES
    ('publication:PUBLIE_RAPPORT', 'publication', 'Publié au rapport',             2),
    ('publication:LETTRE_CHAMBRE', 'publication', 'Publié aux lettres de chambre', 3),
    ('publication:COMMUNIQUE',     'publication', 'Communiqué',                    4);

UPDATE facet_value SET sort = 1 WHERE uid = 'publication:PUBLIE_BULLETIN';
UPDATE facet_value SET sort = 5 WHERE uid = 'publication:INEDIT_BULLETIN';
UPDATE facet_value SET sort = 6 WHERE uid = 'publication:PUBLIE_LEBON';
UPDATE facet_value SET sort = 7 WHERE uid = 'publication:MENTIONNE_LEBON';
UPDATE facet_value SET sort = 8 WHERE uid = 'publication:INEDIT_LEBON';

UPDATE facet_value SET hint = 'Couvre les codes source B, C+ et R.'
WHERE uid = 'publication:MENTIONNE_LEBON';
UPDATE facet_value SET hint = 'Couvre les codes source C, D et Z.'
WHERE uid = 'publication:INEDIT_LEBON';
UPDATE facet_value SET hint = 'Première, deuxième et troisième chambres civiles.'
WHERE uid = 'chamber:CIVILE';
UPDATE facet_value SET hint = 'Chambre commerciale, financière et économique de la Cour de cassation, et chambres commerciales des cours d''appel et des tribunaux.'
WHERE uid = 'chamber:COMMERCIALE';
