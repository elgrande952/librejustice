ALTER TABLE entity ADD COLUMN activity jsonb;

CREATE FUNCTION lj_active_at(activity jsonb, d date) RETURNS boolean
LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
    SELECT activity IS NULL OR d IS NULL OR COALESCE(
        (SELECT bool_or(
                (NOT s ? 'du' OR (s->>'du')::date <= d)
            AND (NOT s ? 'au' OR (s->>'au')::date >= d))
         FROM jsonb_array_elements(activity) s),
        true)
$$;

CREATE TABLE entity_affiliation (
    entity_uid    text NOT NULL,
    structure_uid text NOT NULL,
    du            date,
    au            date,
    source        text NOT NULL
);

CREATE UNIQUE INDEX entity_affiliation_open_idx
    ON entity_affiliation (entity_uid, source) WHERE au IS NULL;
CREATE INDEX entity_affiliation_entity_idx ON entity_affiliation (entity_uid);
CREATE INDEX entity_affiliation_structure_idx ON entity_affiliation (structure_uid);
