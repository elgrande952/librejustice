-- Migration 0204 — Filiation entre textes : copie datée, jamais pointeur.
--
-- Un code reçu naît d'un code parent à une date, par un acte de réception,
-- puis diverge sans fusionner. Aucun événement du parent postérieur au fork
-- n'atteint l'enfant (la renumérotation française de 2016 n'a pas déplacé
-- l'article 1382 belge). La restitution se calcule en LECTURE : les versions
-- du parent antérieures à `fork_date` complètent la timeline de l'enfant,
-- labellisées « héritées » — uniquement si `num_compatible` (Haïti renumérote,
-- FR 1382 = HT 1168 : la fusion y attend une concordance par empreinte).
--
-- FKs vers legal_text bienvenues ici : `text_uid` n'est jamais réécrit en
-- masse, contrairement à `num_key`.

CREATE TABLE text_filiation (
    child_text_uid  text NOT NULL REFERENCES legal_text(text_uid) ON DELETE CASCADE,
    parent_text_uid text NOT NULL REFERENCES legal_text(text_uid) ON DELETE CASCADE,
    fork_date       date,          -- NULL = filiation établie, date non vérifiée
    num_compatible  boolean NOT NULL,
    reception_act   text,
    note            text,
    PRIMARY KEY (child_text_uid, parent_text_uid),
    CHECK (child_text_uid <> parent_text_uid)
);

CREATE INDEX text_filiation_parent_idx ON text_filiation (parent_text_uid);

-- Arêtes vérifiées par source primaire (rapport des forks). Les
-- réceptions non vérifiées (CI, ML, BJ, BF, CF, GA, SN colonial) et le
-- Burundi (petit-fils via le décret belgo-congolais de 1888, absent de la
-- base) attendent leur source — une date fausse restituerait à une branche
-- un passé qui n'est pas le sien.
INSERT INTO text_filiation (child_text_uid, parent_text_uid, fork_date, num_compatible, reception_act, note) VALUES
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
     'Édition « à jour au 1er mai 1956 » ; branche la mieux conservée (1 001+ articles identiques).');
