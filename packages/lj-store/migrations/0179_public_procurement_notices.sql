CREATE TABLE annonce (
    uid              text PRIMARY KEY,
    fond             text NOT NULL,
    famille_uid      text REFERENCES facet_value(uid),
    titre            text NOT NULL,

    acheteur_libelle text NOT NULL,
    departement      text,
    cpv_uid          text REFERENCES facet_value(uid),
    date_publication date NOT NULL,
    echeance         timestamptz,
    closed_at        timestamptz,
    url_action       text NOT NULL,
    details          jsonb NOT NULL

);

CREATE INDEX annonce_ouverte_echeance_idx ON annonce (fond, echeance)
    WHERE closed_at IS NULL;

CREATE TABLE annonce_entity (
    annonce_uid text NOT NULL REFERENCES annonce(uid),
    entity_uid  text NOT NULL,
    role        text NOT NULL,
    PRIMARY KEY (annonce_uid, entity_uid, role)
);

CREATE INDEX annonce_entity_entity_idx ON annonce_entity (entity_uid);

CREATE TABLE annonce_source (
    source_uid       text PRIMARY KEY,
    annonce_uid      text NOT NULL REFERENCES annonce(uid),
    url_avis         text,
    first_seen_at    timestamptz NOT NULL,
    last_seen_at     timestamptz NOT NULL,
    content_checksum bigint NOT NULL
);
CREATE INDEX annonce_source_annonce_idx ON annonce_source (annonce_uid);

CREATE TABLE profil_acheteur (
    host          text PRIMARY KEY,
    moteur        text,
    avis_boamp    int NOT NULL DEFAULT 0,
    first_seen_at timestamptz NOT NULL,
    last_seen_at  timestamptz NOT NULL
);

INSERT INTO facet_value (uid, facet, label, sort) VALUES
    ('nature_marche:TRAVAUX',     'nature_marche', 'Travaux',     1),
    ('nature_marche:SERVICES',    'nature_marche', 'Services',    2),
    ('nature_marche:FOURNITURES', 'nature_marche', 'Fournitures', 3),
    ('nature_marche:AUTRES',      'nature_marche', 'Autres',      4);

INSERT INTO facet_value (uid, facet, label, sort) VALUES
    ('cpv:03', 'cpv', 'Produits agricoles, de l''élevage, de la pêche, de la sylviculture', 3),
    ('cpv:09', 'cpv', 'Produits pétroliers, combustibles, électricité et autres sources d''énergie', 9),
    ('cpv:14', 'cpv', 'Produits des industries extractives, métaux de base et produits connexes', 14),
    ('cpv:15', 'cpv', 'Produits alimentaires, boissons, tabac et produits connexes', 15),
    ('cpv:16', 'cpv', 'Machines agricoles', 16),
    ('cpv:18', 'cpv', 'Vêtements, articles chaussants, bagages et accessoires', 18),
    ('cpv:19', 'cpv', 'Cuir et textiles, matériaux en plastique et en caoutchouc', 19),
    ('cpv:22', 'cpv', 'Imprimés et produits connexes', 22),
    ('cpv:24', 'cpv', 'Produits chimiques', 24),
    ('cpv:30', 'cpv', 'Machines, matériel et fournitures informatiques et de bureau', 30),
    ('cpv:31', 'cpv', 'Machines, appareils, équipements et consommables électriques ; éclairage', 31),
    ('cpv:32', 'cpv', 'Équipements de radio, de télévision, de communication et de télécommunication', 32),
    ('cpv:33', 'cpv', 'Matériels médicaux, produits pharmaceutiques et produits de soins personnels', 33),
    ('cpv:34', 'cpv', 'Équipement de transport et produits auxiliaires pour le transport', 34),
    ('cpv:35', 'cpv', 'Équipement de sécurité, de lutte contre l''incendie, de police et de défense', 35),
    ('cpv:37', 'cpv', 'Instruments de musique, articles de sport, jeux, jouets, artisanat et arts', 37),
    ('cpv:38', 'cpv', 'Équipements de laboratoire, d''optique et de précision', 38),
    ('cpv:39', 'cpv', 'Meubles, aménagements, appareils électroménagers et produits de nettoyage', 39),
    ('cpv:41', 'cpv', 'Eau collectée et purifiée', 41),
    ('cpv:42', 'cpv', 'Machines industrielles', 42),
    ('cpv:43', 'cpv', 'Machines pour l''industrie minière, l''exploitation de carrières et la construction', 43),
    ('cpv:44', 'cpv', 'Structures et matériaux de construction ; produits auxiliaires', 44),
    ('cpv:45', 'cpv', 'Travaux de construction', 45),
    ('cpv:48', 'cpv', 'Logiciels et systèmes d''information', 48),
    ('cpv:50', 'cpv', 'Services de réparation et d''entretien', 50),
    ('cpv:51', 'cpv', 'Services d''installation', 51),
    ('cpv:55', 'cpv', 'Services d''hôtellerie, de restauration et de commerce au détail', 55),
    ('cpv:60', 'cpv', 'Services de transport', 60),
    ('cpv:63', 'cpv', 'Services d''appui et services auxiliaires des transports ; agences de voyages', 63),
    ('cpv:64', 'cpv', 'Services des postes et télécommunications', 64),
    ('cpv:65', 'cpv', 'Services publics', 65),
    ('cpv:66', 'cpv', 'Services financiers et d''assurance', 66),
    ('cpv:70', 'cpv', 'Services immobiliers', 70),
    ('cpv:71', 'cpv', 'Services d''architecture, de construction, d''ingénierie et d''inspection', 71),
    ('cpv:72', 'cpv', 'Services de technologies de l''information', 72),
    ('cpv:73', 'cpv', 'Services de recherche et développement et services de conseil connexes', 73),
    ('cpv:75', 'cpv', 'Services de l''administration publique, de la défense et de la sécurité sociale', 75),
    ('cpv:76', 'cpv', 'Services relatifs à l''industrie du pétrole et du gaz', 76),
    ('cpv:77', 'cpv', 'Services agricoles, sylvicoles, horticoles, d''aquaculture et d''apiculture', 77),
    ('cpv:79', 'cpv', 'Services aux entreprises : droit, marketing, conseil, recrutement, impression', 79),
    ('cpv:80', 'cpv', 'Services d''enseignement et de formation', 80),
    ('cpv:85', 'cpv', 'Services de santé et services sociaux', 85),
    ('cpv:90', 'cpv', 'Services d''assainissement, de déchets, d''hygiénisation et d''environnement', 90),
    ('cpv:92', 'cpv', 'Services récréatifs, culturels et sportifs', 92),
    ('cpv:98', 'cpv', 'Autres services communautaires, sociaux et personnels', 98);

ANALYZE facet_value;
