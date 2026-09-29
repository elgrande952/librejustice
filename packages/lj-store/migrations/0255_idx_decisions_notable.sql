CREATE INDEX idx_decisions_notable ON decisions (id)
WHERE publication_codes && ARRAY['r', 'A', 'b', 'l', 'c', 'B', 'C+', 'R']::text[];
