DROP INDEX chunks_vec;

CREATE INDEX chunks_vec ON decision_chunks
USING vchordrq (embedding rabitq8_cosine_ops)
WITH (options = $$
[build.internal]
lists = [2000]
build_threads = 8
spherical_centroids = true
$$);

ANALYZE decision_chunks;
