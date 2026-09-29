ALTER TABLE llm_credential RENAME COLUMN label TO tenant;

COMMENT ON COLUMN llm_credential.tenant IS 'quota boundary the key belongs to: Mistral organisation, Google project, OpenAI account';
