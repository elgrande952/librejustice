ALTER TABLE chamber ADD COLUMN speciality_uid text;

WITH tot AS (
    SELECT jurisdiction_code AS jc, chamber_key AS ck, count(*) AS n
    FROM decisions
    WHERE deleted_at IS NULL
      AND chamber_key IS NOT NULL AND jurisdiction_code IS NOT NULL
    GROUP BY 1, 2
), par AS (
    SELECT jurisdiction_code AS jc, chamber_key AS ck, matiere_uid AS m, count(*) AS n
    FROM decisions
    WHERE deleted_at IS NULL
      AND chamber_key IS NOT NULL AND jurisdiction_code IS NOT NULL
      AND matiere_uid IS NOT NULL
    GROUP BY 1, 2, 3
), qualifiee AS (
    SELECT t.jc, t.ck, min(p.m) AS m
    FROM tot t
    JOIN par p ON p.jc = t.jc AND p.ck = t.ck
    WHERE t.ck NOT LIKE 'MAT:%'
    GROUP BY t.jc, t.ck, t.n
    HAVING count(*) = 1 AND max(p.n) * 2 >= t.n
)
UPDATE chamber c
SET speciality_uid = q.m
FROM qualifiee q
WHERE c.jurisdiction_code = q.jc AND c.chamber_key = q.ck;
