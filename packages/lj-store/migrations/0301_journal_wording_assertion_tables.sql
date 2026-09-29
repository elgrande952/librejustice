CREATE TABLE journal (
    code       text PRIMARY KEY,
    label      text NOT NULL,
    publisher  text,
    languages  text[] NOT NULL DEFAULT '{fr}'
);

CREATE TABLE wording (
    id                  bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    language            text NOT NULL,
    fingerprint         bigint NOT NULL,
    fingerprint_version smallint NOT NULL,
    length              integer NOT NULL,
    carrier_text_uid    text NOT NULL,
    carrier_num_key     text NOT NULL,
    carrier_start       date NOT NULL,
    carrier_original    boolean NOT NULL DEFAULT false,
    UNIQUE (language, fingerprint, fingerprint_version, length)
);

CREATE TABLE assertion (
    id             bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    text_uid       text NOT NULL,
    num_key        text NOT NULL,
    places         text[] NOT NULL,
    starts_on      date NOT NULL,
    ends_on        date,
    start_origin   text NOT NULL CHECK (start_origin IN ('received', 'computed', 'fallback')),
    end_origin     text CHECK (end_origin IN ('received', 'computed')),
    start_source   text,
    end_source     text,
    version_start  date NOT NULL,
    CHECK (ends_on IS NULL OR ends_on > starts_on),
    CHECK ((ends_on IS NULL) = (end_origin IS NULL)),
    CHECK (cardinality(places) > 0)
);
CREATE INDEX assertion_article_idx ON assertion (text_uid, num_key, starts_on);
CREATE INDEX assertion_places_idx ON assertion USING gin (places);

CREATE TABLE expression (
    assertion_id bigint NOT NULL REFERENCES assertion(id) ON DELETE CASCADE,
    language     text NOT NULL,
    status       text NOT NULL CHECK (status IN ('authentic', 'official', 'translation', 'automatic')),
    wording_id   bigint NOT NULL REFERENCES wording(id),
    source       text,
    PRIMARY KEY (assertion_id, language)
);
CREATE INDEX expression_wording_idx ON expression (wording_id);

CREATE TABLE assertion_compile_state (
    text_uid          text PRIMARY KEY,
    journal_code      text NOT NULL REFERENCES journal(code),
    fingerprint       bigint NOT NULL,
    compiled_at       timestamptz NOT NULL DEFAULT now(),
    versions          integer NOT NULL,
    assertions        integer NOT NULL,
    never_applicable  integer NOT NULL,
    diagnostics       jsonb
);
CREATE INDEX assertion_compile_state_journal_idx ON assertion_compile_state (journal_code);

CREATE VIEW assertion_by_place AS
SELECT a.id, a.text_uid, a.num_key, p.place, a.starts_on, a.ends_on,
       a.start_origin, a.end_origin, a.version_start
FROM assertion a, unnest(a.places) AS p(place);

UPDATE facet_value SET parent_uid = 'geo:FR', label = 'Alsace-Moselle, droit local' WHERE uid = 'geo:FR-ALS';
UPDATE facet_value SET label = 'France métropolitaine, droit général' WHERE uid = 'geo:FR-M';

INSERT INTO facet_value (uid, facet, label, abbr, parent_uid, sort) VALUES
    ('geo:CH-AG', 'geo', 'Argovie',                   'AG', 'geo:CH', 200),
    ('geo:CH-AI', 'geo', 'Appenzell Rhodes-Intérieures', 'AI', 'geo:CH', 201),
    ('geo:CH-AR', 'geo', 'Appenzell Rhodes-Extérieures', 'AR', 'geo:CH', 202),
    ('geo:CH-BE', 'geo', 'Berne',                     'BE', 'geo:CH', 203),
    ('geo:CH-BL', 'geo', 'Bâle-Campagne',             'BL', 'geo:CH', 204),
    ('geo:CH-BS', 'geo', 'Bâle-Ville',                'BS', 'geo:CH', 205),
    ('geo:CH-FR', 'geo', 'Fribourg',                  'FR', 'geo:CH', 206),
    ('geo:CH-GE', 'geo', 'Genève',                    'GE', 'geo:CH', 207),
    ('geo:CH-GL', 'geo', 'Glaris',                    'GL', 'geo:CH', 208),
    ('geo:CH-GR', 'geo', 'Grisons',                   'GR', 'geo:CH', 209),
    ('geo:CH-JU', 'geo', 'Jura',                      'JU', 'geo:CH', 210),
    ('geo:CH-LU', 'geo', 'Lucerne',                   'LU', 'geo:CH', 211),
    ('geo:CH-NE', 'geo', 'Neuchâtel',                 'NE', 'geo:CH', 212),
    ('geo:CH-NW', 'geo', 'Nidwald',                   'NW', 'geo:CH', 213),
    ('geo:CH-OW', 'geo', 'Obwald',                    'OW', 'geo:CH', 214),
    ('geo:CH-SG', 'geo', 'Saint-Gall',                'SG', 'geo:CH', 215),
    ('geo:CH-SH', 'geo', 'Schaffhouse',               'SH', 'geo:CH', 216),
    ('geo:CH-SO', 'geo', 'Soleure',                   'SO', 'geo:CH', 217),
    ('geo:CH-SZ', 'geo', 'Schwytz',                   'SZ', 'geo:CH', 218),
    ('geo:CH-TG', 'geo', 'Thurgovie',                 'TG', 'geo:CH', 219),
    ('geo:CH-TI', 'geo', 'Tessin',                    'TI', 'geo:CH', 220),
    ('geo:CH-UR', 'geo', 'Uri',                       'UR', 'geo:CH', 221),
    ('geo:CH-VD', 'geo', 'Vaud',                      'VD', 'geo:CH', 222),
    ('geo:CH-VS', 'geo', 'Valais',                    'VS', 'geo:CH', 223),
    ('geo:CH-ZG', 'geo', 'Zoug',                      'ZG', 'geo:CH', 224),
    ('geo:CH-ZH', 'geo', 'Zurich',                    'ZH', 'geo:CH', 225),
    ('geo:BE-BRU', 'geo', 'Région de Bruxelles-Capitale',       NULL, 'geo:BE', 230),
    ('geo:BE-VLG', 'geo', 'Région flamande',                    NULL, 'geo:BE', 231),
    ('geo:BE-WAL', 'geo', 'Région wallonne',                    NULL, 'geo:BE', 232),
    ('geo:BE-CFR', 'geo', 'Communauté française',               NULL, 'geo:BE', 233),
    ('geo:BE-CNL', 'geo', 'Communauté flamande',                NULL, 'geo:BE', 234),
    ('geo:BE-CDE', 'geo', 'Communauté germanophone',            NULL, 'geo:BE', 235),
    ('geo:CY', 'geo', 'Chypre',     NULL, NULL, 160),
    ('geo:CZ', 'geo', 'Tchéquie',   NULL, NULL, 161),
    ('geo:DK', 'geo', 'Danemark',   NULL, NULL, 162),
    ('geo:EE', 'geo', 'Estonie',    NULL, NULL, 163),
    ('geo:FI', 'geo', 'Finlande',   NULL, NULL, 164),
    ('geo:HR', 'geo', 'Croatie',    NULL, NULL, 165),
    ('geo:IE', 'geo', 'Irlande',    NULL, NULL, 166),
    ('geo:LT', 'geo', 'Lituanie',   NULL, NULL, 167),
    ('geo:LV', 'geo', 'Lettonie',   NULL, NULL, 168),
    ('geo:MT', 'geo', 'Malte',      NULL, NULL, 169),
    ('geo:SE', 'geo', 'Suède',      NULL, NULL, 170),
    ('geo:SI', 'geo', 'Slovénie',   NULL, NULL, 171),
    ('geo:SK', 'geo', 'Slovaquie',  NULL, NULL, 172),
    ('geo:GB', 'geo', 'Royaume-Uni', NULL, NULL, 173);
