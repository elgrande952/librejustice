CREATE TABLE entity (
    uid                 text PRIMARY KEY,
    nature              text NOT NULL CHECK (nature IN
                            ('morale_privee', 'morale_publique', 'physique')),
    denomination        text NOT NULL,
    denomination_folded text NOT NULL,
    sigle               text,
    forme               text,
    active              bool NOT NULL,
    updated_at          timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX entity_denomination_folded_idx ON entity (denomination_folded);
ALTER TABLE entity SET (fillfactor = 100,
    autovacuum_vacuum_scale_factor = 0.02, autovacuum_analyze_scale_factor = 0.01);

CREATE TABLE entity_denomination (
    entity_uid   text NOT NULL REFERENCES entity(uid) ON DELETE CASCADE,
    denomination text NOT NULL,
    folded       text NOT NULL,
    date_debut   date,
    date_fin     date
);
CREATE INDEX entity_denomination_uid_idx ON entity_denomination (entity_uid);
CREATE INDEX entity_denomination_resolve_idx ON entity_denomination (folded);
ALTER TABLE entity_denomination SET (fillfactor = 100,
    autovacuum_vacuum_scale_factor = 0.02, autovacuum_analyze_scale_factor = 0.01);
