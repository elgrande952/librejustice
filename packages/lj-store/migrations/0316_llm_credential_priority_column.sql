ALTER TABLE comptes.llm_credential ADD COLUMN priority SMALLINT NOT NULL DEFAULT 0;

COMMENT ON COLUMN comptes.llm_credential.priority IS
    'service rank inside the pool, lowest served first; ties fall back to what the door announces';
