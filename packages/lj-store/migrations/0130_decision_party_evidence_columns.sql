ALTER TABLE decision_party
    ADD COLUMN char_starts int4[],
    ADD COLUMN char_ends int4[],
    ADD COLUMN nature text CHECK (nature IN
        ('physique', 'morale_privee', 'morale_publique')),
    ADD COLUMN extract_version int2 NOT NULL DEFAULT 0;
ALTER TABLE decision_party ALTER COLUMN extract_version DROP DEFAULT;

DROP INDEX decision_party_pending_idx;
CREATE INDEX decision_party_pending_idx ON decision_party (resolve_key)
    WHERE entity_uid IS NULL AND quality IN ('party', 'law_firm');
