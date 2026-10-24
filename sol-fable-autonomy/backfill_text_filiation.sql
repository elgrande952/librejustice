-- Explicit post-ingestion backfill; safe to rerun. Does not insert legal_text rows.
-- Values preserved from LibreJustice ca1d06062ec4bb2cb332b0e9d41a45309fa3ad75, Apache-2.0.
WITH filiation_seed (child_text_uid, parent_text_uid, fork_date, num_compatible, reception_act, note) AS (
    VALUES
    ('BE/CODE-CIVIL', 'LEGITEXT000006070721', '1815-06-09', true,
     'Loi du 9 vendémiaire an IV (annexion) ; code du 21 mars 1804',
     'Divergence à la fin de la souveraineté française (acte final du Congrès de Vienne).'),
    ('LU/CODE-CIVIL', 'LEGITEXT000006070721', '1815-06-09', true,
     'Loi du 9 vendémiaire an IV (département des Forêts)',
     'Divergence à la création du Grand-Duché (Congrès de Vienne).'),
    ('HT/CODE-CIVIL', 'LEGITEXT000006070721', '1826-05-01', false,
     'Code civil haïtien de 1825, art. 2047 (en vigueur le 1er mai 1826)',
     'Calqué sur l''état 1804 pré-Restauration, RENUMÉROTÉ (FR 1382 = HT 1168) : fusion de timeline interdite sans concordance.'),
    ('MU/CODE-CIVIL', 'LEGITEXT000006070721', '1810-12-03', true,
     'Décret de Decaen du 21 avril 1808 (Code Napoléon)',
     'Gel à la capitulation britannique ; regreffes mauriciennes ultérieures sur du droit français du XXe siècle.'),
    ('CM/CODE-CIVIL', 'LEGITEXT000006070721', '1960-01-01', true,
     'Décret du 22 mai 1924 (extension de la législation AEF)',
     'Gel à l''indépendance ; promulgations locales tracées dans le texte (B.A.S. 1856…).'),
    ('TD/CODE-CIVIL', 'LEGITEXT000006070721', '1959-03-31', true,
     'Acte législatif n° 1 portant constitution du 31 mars 1959 ; loi constitutionnelle n° 2/62 du 16 avril 1962',
     'Code français de 1958 gelé ; greffes locales datées article par article (J.O. A.E.F. 1913…).'),
    ('TG/CODE-CIVIL', 'LEGITEXT000006070721', '1956-05-01', true,
     'Continuité : loi organique du 23 avril 1960, art. 31',
     'Édition « à jour au 1er mai 1956 » ; branche la mieux conservée (1 001+ articles identiques).')
)
INSERT INTO text_filiation (child_text_uid, parent_text_uid, fork_date, num_compatible, reception_act, note)
SELECT s.child_text_uid, s.parent_text_uid, s.fork_date::date, s.num_compatible, s.reception_act, s.note
FROM filiation_seed AS s
WHERE EXISTS (SELECT 1 FROM legal_text t WHERE t.text_uid = s.child_text_uid)
  AND EXISTS (SELECT 1 FROM legal_text t WHERE t.text_uid = s.parent_text_uid)
ON CONFLICT (child_text_uid, parent_text_uid) DO NOTHING;
