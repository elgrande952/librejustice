CREATE TABLE citing_decision_counts (
    cited_term     text   PRIMARY KEY,
    decision_count bigint NOT NULL
);

INSERT INTO citing_decision_counts (cited_term, decision_count)
SELECT term, count(*)
FROM legal_citation, unnest(public.lj_cit_terms(spans)) AS term
GROUP BY term;
