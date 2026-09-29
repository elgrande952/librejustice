CREATE TABLE article_scope (
    text_uid   text NOT NULL,
    num_key    text NOT NULL,
    date_debut date NOT NULL,
    mode       text NOT NULL CHECK (mode IN ('etend', 'ecarte', 'restreint')),
    scope      text NOT NULL REFERENCES facet_value(uid),
    s_debut    date NOT NULL,
    s_fin      date,
    source     text,
    PRIMARY KEY (text_uid, num_key, date_debut, mode, scope, s_debut),
    CHECK (s_fin IS NULL OR s_fin > s_debut),
    CHECK (split_part(scope, ':', 1) IN ('geo', 'natio', 'statut', 'communaute'))
);

COMMENT ON TABLE article_scope IS
  'Événements d''applicabilité d''une version d''article. Absence '
  'de ligne = domaine par défaut de la juridiction du texte. « Applicable à '
  'D dans S » : aucun ecarte couvrant (D, S ou ancêtre) ET (un etend/restreint '
  'couvre (D, S ou ancêtre) OU (pas de restreint actif ET D dans '
  '[date_debut, date_fin) ET S dans le domaine par défaut)).';

INSERT INTO facet_value (uid, facet, label, abbr, parent_uid, sort) VALUES
    ('geo:FR',     'geo', 'France (ordre juridique)',  NULL,   NULL,     0),
    ('geo:FR-M',   'geo', 'France métropolitaine',     NULL,   'geo:FR', 1),
    ('geo:FR-ALS', 'geo', 'Alsace-Moselle',            NULL,   'geo:FR-M', 2),
    ('geo:GP',     'geo', 'Guadeloupe',                NULL,   'geo:FR', 10),
    ('geo:MQ',     'geo', 'Martinique',                NULL,   'geo:FR', 11),
    ('geo:GF',     'geo', 'Guyane',                    NULL,   'geo:FR', 12),
    ('geo:RE',     'geo', 'La Réunion',                NULL,   'geo:FR', 13),
    ('geo:YT',     'geo', 'Mayotte',                   NULL,   'geo:FR', 14),
    ('geo:PM',     'geo', 'Saint-Pierre-et-Miquelon',  'SPM',  'geo:FR', 20),
    ('geo:BL',     'geo', 'Saint-Barthélemy',          NULL,   'geo:FR', 21),
    ('geo:MF',     'geo', 'Saint-Martin',              NULL,   'geo:FR', 22),
    ('geo:NC',     'geo', 'Nouvelle-Calédonie',        NULL,   'geo:FR', 30),
    ('geo:PF',     'geo', 'Polynésie française',       NULL,   'geo:FR', 31),
    ('geo:WF',     'geo', 'Wallis-et-Futuna',          NULL,   'geo:FR', 32),
    ('geo:TF',     'geo', 'Terres australes',          'TAAF', 'geo:FR', 33);
