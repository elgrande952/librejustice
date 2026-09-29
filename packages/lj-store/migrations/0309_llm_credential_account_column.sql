ALTER TABLE llm_credential ADD COLUMN account text;

COMMENT ON COLUMN llm_credential.account IS 'console login that holds the tenant: Google account, Mistral account, OpenAI account';
