-- Pot « Annonces » : fond marchés publics en table.
--
-- Quatrième nature de l'ontologie : l'événement juridique en cours, publié
-- obligatoirement et appelant une action avant une échéance. Les fonds servis
-- par une API publique (BODACC, JOAFE) restent en proxy live ; les
-- marchés publics entrent en table parce que la requête produit — liste
-- unifiée BOAMP + profils d'acheteurs, dédupliquée inter-plateformes,
-- facettée — n'est servie par aucune API. Mesures (working note 2026-08-04) :
-- stock actif 14 863 consultations France entière, flux ~200-300 k/an,
-- ~1 Go/an tout compris — deux ordres de grandeur sous le scatter 0197
-- (registre_annonce, 59 Go, droppée en 0139). Garde-fous : une colonne
-- n'existe que si c'est un axe de facette/tri/alerte de /annonces, `details`
-- ne porte jamais le payload source brut, et le cron publie la taille des
-- tables dans la télémétrie.

-- La ligne canonique : un événement, quel que soit le nombre de canaux qui le
-- publient. uid = 'marche:<xxh3 de la clé canonique>' (clé : acheteur plié +
-- référence acheteur ; repli acheteur plié + échéance + objet plié tronqué).
CREATE TABLE annonce (
    uid              text PRIMARY KEY,
    fond             text NOT NULL,             -- 'marche' ; demain 'enquete_publique'…
    famille_uid      text REFERENCES facet_value(uid),
    titre            text NOT NULL,
    -- Libellé source de l'acheteur : affichable même sans rattachement au
    -- graphe (le rattachement entité vit dans annonce_entity, faillible).
    acheteur_libelle text NOT NULL,
    departement      text,
    cpv_uid          text REFERENCES facet_value(uid),
    date_publication date NOT NULL,
    echeance         timestamptz,               -- l'axe : tri par défaut, alertes, fenêtre
    closed_at        timestamptz,
    url_action       text NOT NULL,             -- où agir : déposer / enchérir / observer
    details          jsonb NOT NULL             -- le spécifique du fond : liens RC/DCE/dépôt,
                                                -- référence, procédure, CPV complet (~0,5-1 Ko)
);

-- La fenêtre vivante de /annonces : filtres facettes + tri par échéance sur
-- les lignes ouvertes (~15 k), les closes (l'archive) hors index.
CREATE INDEX annonce_ouverte_echeance_idx ON annonce (fond, echeance)
    WHERE closed_at IS NULL;

-- Arêtes vers le graphe, typées par rôle : l'acheteur d'un marché puis ses
-- titulaires à l'attribution. Pas de FK vers entity — les référentiels se
-- rechargent par remplacement de namespace (doctrine 0135/0178).
CREATE TABLE annonce_entity (
    annonce_uid text NOT NULL REFERENCES annonce(uid),
    entity_uid  text NOT NULL,                  -- 'siren:…'
    role        text NOT NULL,                  -- acheteur | titulaire | débiteur…
    PRIMARY KEY (annonce_uid, entity_uid, role)
);
-- La fiche entité requête exactement (entity_uid, role).
CREATE INDEX annonce_entity_entity_idx ON annonce_entity (entity_uid);

-- Un canal de publication par ligne (BOAMP, moteur de profil d'acheteur).
-- Le moteur se lit dans le préfixe du source_uid ('boamp:…', 'aws:…') ; le
-- cycle de vie s'appuie sur last_seen_at : une consultation disparue d'un
-- moteur dont le run a passé son sanity check ferme sa ligne canonique.
CREATE TABLE annonce_source (
    source_uid       text PRIMARY KEY,
    annonce_uid      text NOT NULL REFERENCES annonce(uid),
    url_avis         text,
    first_seen_at    timestamptz NOT NULL,
    last_seen_at     timestamptz NOT NULL,
    content_checksum bigint NOT NULL            -- xxh3-64 du payload source brut
);
CREATE INDEX annonce_source_annonce_idx ON annonce_source (annonce_uid);

-- Recensement des profils d'acheteurs (annonces-discover) : les hôtes cités
-- par les avis BOAMP, classés par moteur (domaine, sonde Hydra, empreinte
-- HTML). Les moteurs multi-tenants (atexo, safetender) énumèrent les hôtes
-- recensés ici ; avis_boamp pondère (hôte cité une fois ≠ plateforme).
CREATE TABLE profil_acheteur (
    host          text PRIMARY KEY,
    moteur        text,                         -- NULL = non identifié
    avis_boamp    int NOT NULL DEFAULT 0,
    first_seen_at timestamptz NOT NULL,
    last_seen_at  timestamptz NOT NULL
);

-- ── Vocabulaire de facettes du fond marchés ────────────────────────────────

-- Nature de marché (le formulaire commun aux 9 moteurs et au BOAMP).
INSERT INTO facet_value (uid, facet, label, sort) VALUES
    ('nature_marche:TRAVAUX',     'nature_marche', 'Travaux',     1),
    ('nature_marche:SERVICES',    'nature_marche', 'Services',    2),
    ('nature_marche:FOURNITURES', 'nature_marche', 'Fournitures', 3),
    ('nature_marche:AUTRES',      'nature_marche', 'Autres',      4);

-- Divisions CPV (règlement CE 213/2008) : le grain de facette de /annonces.
-- Le code CPV complet de l'avis vit dans annonce.details.
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
