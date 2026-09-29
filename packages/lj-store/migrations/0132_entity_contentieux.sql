CREATE TABLE entity_contentieux (
    entity_uid          text PRIMARY KEY REFERENCES entity(uid) ON DELETE CASCADE,

    category            text NOT NULL,
    namespace           text NOT NULL,
    nature              text NOT NULL,
    denomination        text NOT NULL,
    denomination_folded text NOT NULL,
    forme               text,
    active              bool NOT NULL,

    barreau             text,
    decision_count      bigint NOT NULL
);

CREATE INDEX entity_contentieux_listing_idx
    ON entity_contentieux (category, decision_count DESC, denomination_folded);

CREATE INDEX entity_contentieux_barreau_idx
    ON entity_contentieux (barreau, decision_count DESC, denomination_folded)
    WHERE barreau IS NOT NULL;

CREATE INDEX entity_contentieux_search_idx
    ON entity_contentieux (denomination_folded text_pattern_ops);

ALTER TABLE entity_contentieux SET (fillfactor = 100,
    autovacuum_vacuum_scale_factor = 0.02, autovacuum_analyze_scale_factor = 0.01);
