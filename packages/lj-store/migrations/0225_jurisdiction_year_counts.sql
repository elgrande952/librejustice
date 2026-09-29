CREATE TABLE jurisdiction_year_counts (
    jurisdiction_code text   NOT NULL,
    year              int,
    decision_count    bigint NOT NULL,
    CONSTRAINT jurisdiction_year_counts_key
        UNIQUE NULLS NOT DISTINCT (jurisdiction_code, year)
);

INSERT INTO jurisdiction_year_counts (jurisdiction_code, year, decision_count)
SELECT jurisdiction_code, extract(year FROM date_lecture)::int, count(*)
FROM decisions
WHERE deleted_at IS NULL
GROUP BY 1, 2;
