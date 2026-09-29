CREATE TABLE scope_membership (
    member_scope    text NOT NULL REFERENCES facet_value(uid),
    community_scope text NOT NULL REFERENCES facet_value(uid),
    m_debut         date NOT NULL,
    m_fin           date,
    source          text,
    PRIMARY KEY (member_scope, community_scope, m_debut),
    CHECK (m_fin IS NULL OR m_fin > m_debut),
    CHECK (split_part(community_scope, ':', 1) = 'communaute')
);

ALTER TABLE legal_text ADD COLUMN default_scope text REFERENCES facet_value(uid);

INSERT INTO facet_value (uid, facet, label, abbr, parent_uid, sort) VALUES
    ('communaute:OHADA', 'communaute', 'OHADA (droit uniforme des affaires)', 'OHADA', NULL, 0),
    ('communaute:UEMOA', 'communaute', 'Union économique et monétaire ouest-africaine', 'UEMOA', NULL, 1),
    ('communaute:CIMA',  'communaute', 'Conférence interafricaine des marchés d''assurances', 'CIMA', NULL, 2),
    ('communaute:CEMAC', 'communaute', 'Communauté économique et monétaire de l''Afrique centrale', 'CEMAC', NULL, 3);

INSERT INTO facet_value (uid, facet, label, abbr, parent_uid, sort) VALUES
    ('geo:AM', 'geo', 'Arménie',                 NULL, NULL, 100),
    ('geo:AO', 'geo', 'Angola',                  NULL, NULL, 101),
    ('geo:AT', 'geo', 'Autriche',                NULL, NULL, 102),
    ('geo:BE', 'geo', 'Belgique',                NULL, NULL, 103),
    ('geo:BF', 'geo', 'Burkina Faso',            NULL, NULL, 104),
    ('geo:BG', 'geo', 'Bulgarie',                NULL, NULL, 105),
    ('geo:BI', 'geo', 'Burundi',                 NULL, NULL, 106),
    ('geo:BJ', 'geo', 'Bénin',                   NULL, NULL, 107),
    ('geo:CD', 'geo', 'République démocratique du Congo', 'RDC', NULL, 108),
    ('geo:CF', 'geo', 'République centrafricaine', 'RCA', NULL, 109),
    ('geo:CG', 'geo', 'Congo',                   NULL, NULL, 110),
    ('geo:CH', 'geo', 'Suisse',                  NULL, NULL, 111),
    ('geo:CI', 'geo', 'Côte d''Ivoire',          NULL, NULL, 112),
    ('geo:CM', 'geo', 'Cameroun',                NULL, NULL, 113),
    ('geo:DE', 'geo', 'Allemagne',               NULL, NULL, 114),
    ('geo:DJ', 'geo', 'Djibouti',                NULL, NULL, 115),
    ('geo:DO', 'geo', 'République dominicaine',  NULL, NULL, 116),
    ('geo:DZ', 'geo', 'Algérie',                 NULL, NULL, 117),
    ('geo:EG', 'geo', 'Égypte',                  NULL, NULL, 118),
    ('geo:ES', 'geo', 'Espagne',                 NULL, NULL, 119),
    ('geo:GA', 'geo', 'Gabon',                   NULL, NULL, 120),
    ('geo:GN', 'geo', 'Guinée',                  NULL, NULL, 121),
    ('geo:GR', 'geo', 'Grèce',                   NULL, NULL, 122),
    ('geo:HT', 'geo', 'Haïti',                   NULL, NULL, 123),
    ('geo:HU', 'geo', 'Hongrie',                 NULL, NULL, 124),
    ('geo:IQ', 'geo', 'Irak',                    NULL, NULL, 125),
    ('geo:IT', 'geo', 'Italie',                  NULL, NULL, 126),
    ('geo:JO', 'geo', 'Jordanie',                NULL, NULL, 127),
    ('geo:KM', 'geo', 'Comores',                 NULL, NULL, 128),
    ('geo:LB', 'geo', 'Liban',                   NULL, NULL, 129),
    ('geo:LU', 'geo', 'Luxembourg',              NULL, NULL, 130),
    ('geo:MA', 'geo', 'Maroc',                   NULL, NULL, 131),
    ('geo:MC', 'geo', 'Monaco',                  NULL, NULL, 132),
    ('geo:MG', 'geo', 'Madagascar',              NULL, NULL, 133),
    ('geo:ML', 'geo', 'Mali',                    NULL, NULL, 134),
    ('geo:MR', 'geo', 'Mauritanie',              NULL, NULL, 135),
    ('geo:MU', 'geo', 'Maurice',                 NULL, NULL, 136),
    ('geo:NE', 'geo', 'Niger',                   NULL, NULL, 137),
    ('geo:NG', 'geo', 'Nigéria',                 NULL, NULL, 138),
    ('geo:NL', 'geo', 'Pays-Bas',                NULL, NULL, 139),
    ('geo:PE', 'geo', 'Pérou',                   NULL, NULL, 140),
    ('geo:PL', 'geo', 'Pologne',                 NULL, NULL, 141),
    ('geo:PT', 'geo', 'Portugal',                NULL, NULL, 142),
    ('geo:RO', 'geo', 'Roumanie',                NULL, NULL, 143),
    ('geo:RS', 'geo', 'Serbie',                  NULL, NULL, 144),
    ('geo:RU', 'geo', 'Russie',                  NULL, NULL, 145),
    ('geo:RW', 'geo', 'Rwanda',                  NULL, NULL, 146),
    ('geo:SN', 'geo', 'Sénégal',                 NULL, NULL, 147),
    ('geo:ST', 'geo', 'Sao Tomé-et-Principe',    NULL, NULL, 148),
    ('geo:SY', 'geo', 'Syrie',                   NULL, NULL, 149),
    ('geo:TD', 'geo', 'Tchad',                   NULL, NULL, 150),
    ('geo:TG', 'geo', 'Togo',                    NULL, NULL, 151),
    ('geo:TN', 'geo', 'Tunisie',                 NULL, NULL, 152),
    ('geo:TR', 'geo', 'Turquie',                 NULL, NULL, 153),
    ('geo:UA', 'geo', 'Ukraine',                 NULL, NULL, 154),
    ('geo:VE', 'geo', 'Venezuela',               NULL, NULL, 155),
    ('geo:VN', 'geo', 'Viêt Nam',                NULL, NULL, 156);

INSERT INTO scope_membership (member_scope, community_scope, m_debut, m_fin, source) VALUES
    ('geo:SN', 'communaute:OHADA', '1995-09-18', NULL,
     'Traité de Port-Louis du 17 octobre 1993, entré en vigueur le 18 septembre 1995 ; Sénégal ratifié le 14 juin 1994'),
    ('geo:CD', 'communaute:OHADA', '2012-09-12', NULL,
     'Adhésion de la RDC, entrée en vigueur le 12 septembre 2012');

UPDATE legal_text SET default_scope = 'geo:FR-ALS'
WHERE text_uid IN ('JORFTEXT000000869866', 'JORFTEXT000000869867');
UPDATE legal_text SET default_scope = 'geo:PF'
WHERE text_uid IN ('JORFTEXT000024961711', 'JORFTEXT000024961743');
