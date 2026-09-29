CREATE TABLE registre_annonce (
    id             BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,

    source_uid     TEXT NOT NULL UNIQUE,

    source_file    TEXT NOT NULL,
    registre       TEXT NOT NULL,
    famille        TEXT NOT NULL,
    type_avis      TEXT NOT NULL,
    entity_uids    TEXT[] NOT NULL DEFAULT '{}',
    denomination   TEXT,
    tribunal       TEXT,
    date_evenement DATE,
    date_parution  DATE NOT NULL,
    parution       TEXT,
    numero_annonce INTEGER,
    details        JSONB NOT NULL DEFAULT '{}'::jsonb
);

CREATE INDEX registre_annonce_entity_uids_idx ON registre_annonce USING gin (entity_uids);

CREATE INDEX registre_annonce_famille_idx ON registre_annonce (registre, famille);

CREATE INDEX registre_annonce_source_file_idx ON registre_annonce (source_file);

CREATE TABLE registre_annonce_file (
    file_name   TEXT PRIMARY KEY,
    checksum    BIGINT NOT NULL,
    annonces    INTEGER NOT NULL,
    ingested_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
