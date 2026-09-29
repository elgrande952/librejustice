CREATE TABLE decision_party (
    decision_id  bigint NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    ord          int    NOT NULL,
    quality      text   NOT NULL CHECK (quality IN
                     ('party', 'law_firm', 'counsel_name', 'intervenor')),
    side         text   CHECK (side IN ('applicant', 'defendant')),
    value        text   NOT NULL,
    resolve_key  text   NOT NULL,
    entity_uid   text,
    PRIMARY KEY (decision_id, ord)
);

CREATE INDEX decision_party_pending_idx ON decision_party (resolve_key)
    WHERE entity_uid IS NULL AND quality IN ('party', 'intervenor', 'law_firm');

CREATE INDEX decision_party_entity_idx ON decision_party (entity_uid)
    WHERE entity_uid IS NOT NULL;
