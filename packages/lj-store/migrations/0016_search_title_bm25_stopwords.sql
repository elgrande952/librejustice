DROP INDEX IF EXISTS decisions_title_bm25;

CREATE INDEX decisions_title_bm25 ON decisions
USING bm25 (id, (search_title::pdb.simple('stopwords_language=French', 'ascii_folding=true')))
WITH (key_field = 'id');
