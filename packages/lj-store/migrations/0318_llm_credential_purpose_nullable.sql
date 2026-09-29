ALTER TABLE comptes.llm_credential ALTER COLUMN purpose DROP NOT NULL;

DROP INDEX comptes.llm_credential_pool;

CREATE INDEX llm_credential_pool ON comptes.llm_credential (provider);
