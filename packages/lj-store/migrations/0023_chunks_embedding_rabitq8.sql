DROP INDEX IF EXISTS chunks_vec;

ALTER TABLE decision_chunks
    ALTER COLUMN embedding TYPE rabitq8(1024)
    USING quantize_to_rabitq8(embedding)::rabitq8(1024);

CREATE INDEX chunks_vec ON decision_chunks
USING vchordrq (embedding rabitq8_cosine_ops);
