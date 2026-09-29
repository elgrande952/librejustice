CREATE INDEX IF NOT EXISTS idx_tlc_ref ON text_legal_citation (ref_text_uid);

DELETE FROM legal_link ll
USING legal_text lt
WHERE ll.owner_text_uid = lt.text_uid
  AND lt.text_uid LIKE 'LEGITEXT%'
  AND (lt.body IS NULL OR length(lt.body) < 300)
  AND NOT EXISTS (SELECT 1 FROM legal_article la WHERE la.text_uid = lt.text_uid);

UPDATE legal_citation lc SET ref_text_uid = NULL
FROM legal_text lt
WHERE lc.ref_text_uid = lt.text_uid
  AND lt.text_uid LIKE 'LEGITEXT%'
  AND (lt.body IS NULL OR length(lt.body) < 300)
  AND NOT EXISTS (SELECT 1 FROM legal_article la WHERE la.text_uid = lt.text_uid);

DELETE FROM legal_text lt
WHERE lt.text_uid LIKE 'LEGITEXT%'
  AND (lt.body IS NULL OR length(lt.body) < 300)
  AND NOT EXISTS (SELECT 1 FROM legal_article la WHERE la.text_uid = lt.text_uid);
