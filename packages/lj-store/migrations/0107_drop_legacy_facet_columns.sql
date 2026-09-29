SET LOCAL maintenance_work_mem = '2GB';
SET LOCAL max_parallel_maintenance_workers = 4;

CREATE OR REPLACE FUNCTION sync_chunks_decision_meta()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    UPDATE decision_chunks SET
        jurisdiction_name  = NEW.jurisdiction_name,
        publication_codes  = NEW.publication_codes,
        date_lecture       = NEW.date_lecture,
        search_title       = CASE WHEN chunk_index = 0 THEN NEW.search_title END
    WHERE decision_id = NEW.id;
    RETURN NULL;
END;
$$;

DROP TRIGGER IF EXISTS decisions_sync_chunks_meta ON decisions;
CREATE TRIGGER decisions_sync_chunks_meta
AFTER UPDATE ON decisions
FOR EACH ROW
WHEN (
    OLD.jurisdiction_name IS DISTINCT FROM NEW.jurisdiction_name
    OR OLD.publication_codes IS DISTINCT FROM NEW.publication_codes
    OR OLD.date_lecture IS DISTINCT FROM NEW.date_lecture
    OR OLD.search_title IS DISTINCT FROM NEW.search_title
)
EXECUTE FUNCTION sync_chunks_decision_meta();

DROP INDEX IF EXISTS idx_chunks_jurisdiction_level;
DROP INDEX IF EXISTS idx_chunks_instance_level;
DROP INDEX IF EXISTS idx_chunks_main_outcome;
DROP INDEX IF EXISTS idx_chunks_special_procedure;
ALTER TABLE decision_chunks
    DROP COLUMN IF EXISTS jurisdiction_level,
    DROP COLUMN IF EXISTS instance_level,
    DROP COLUMN IF EXISTS main_outcome,
    DROP COLUMN IF EXISTS special_procedure;

DROP INDEX IF EXISTS decisions_bm25;

DROP INDEX IF EXISTS idx_decisions_jurisdiction_level;
DROP INDEX IF EXISTS idx_decisions_instance_level;
DROP INDEX IF EXISTS idx_decisions_main_outcome;
DROP INDEX IF EXISTS idx_decisions_special_procedure;
ALTER TABLE decisions
    DROP COLUMN IF EXISTS jurisdiction_level,
    DROP COLUMN IF EXISTS instance_level,
    DROP COLUMN IF EXISTS main_outcome,
    DROP COLUMN IF EXISTS special_procedure,
    DROP COLUMN IF EXISTS legal_domain,
    DROP COLUMN IF EXISTS instance_uid,
    DROP COLUMN IF EXISTS defendant_administration;

DELETE FROM facet_value WHERE uid LIKE 'instance:%';

CREATE INDEX decisions_bm25 ON decisions
USING bm25 (
    id,
    full_text,
    search_title,
    (juridiction_type::pdb.literal),
    (jurisdiction_name::pdb.literal),
    (legal_instruments::pdb.literal),
    (legal_article_composite::pdb.literal),
    (publication_codes::pdb.literal),
    date_lecture
)
WITH (
  key_field = 'id',
  text_fields = '{
    "full_text":    {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"},
    "search_title": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à", "above", "after", "again", "against", "all", "also", "am", "and", "any", "at", "because", "been", "before", "being", "below", "between", "both", "by", "cannot", "could", "did", "does", "doing", "down", "during", "each", "few", "from", "further", "had", "has", "having", "hence", "her", "hers", "herself", "him", "himself", "his", "how", "however", "into", "is", "it", "its", "itself", "most", "my", "myself", "not", "of", "only", "other", "ought", "our", "ourselves", "over", "own", "same", "she", "should", "so", "some", "such", "than", "that", "their", "theirs", "them", "themselves", "then", "there", "therefore", "they", "this", "those", "through", "thus", "to", "too", "under", "until", "up", "very", "was", "we", "were", "what", "when", "where", "whereas", "which", "while", "who", "whom", "why", "with", "would", "you", "your", "yours", "yourself", "yourselves"]}, "record": "position"}
  }'
);

ANALYZE decisions;
ANALYZE decision_chunks;
