ALTER TABLE decision_party ADD COLUMN barreau text;

DROP INDEX decision_party_pending_idx;
CREATE INDEX decision_party_pending_idx ON decision_party (resolve_key)
    WHERE entity_uid IS NULL
      AND quality IN ('party', 'law_firm', 'counsel_name');
