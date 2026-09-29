UPDATE decisions SET jurisdiction_code = 'cc'
WHERE juridiction_type = 'CC' AND jurisdiction_code <> 'cc';

DELETE FROM jurisdiction WHERE code LIKE 'cass\_%';

ANALYZE decisions;
