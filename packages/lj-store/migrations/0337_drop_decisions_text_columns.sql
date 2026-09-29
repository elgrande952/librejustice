DROP TRIGGER decisions_mirror_text ON decisions;

CREATE OR REPLACE FUNCTION decision_text_mirror(p_ids BIGINT[]) RETURNS VOID LANGUAGE plpgsql AS $$
DECLARE
    v_id BIGINT;
    v_text TEXT;
BEGIN
    UPDATE decision_text t SET
        search_title = d.search_title,
        jurisdiction_type = d.jurisdiction_type,
        jurisdiction_code = d.jurisdiction_code,
        seat_paths = d.seat_paths,
        legal_instruments = d.legal_instruments,
        legal_article_composite = d.legal_article_composite,
        publication_codes = d.publication_codes,
        legal_domain_uid = d.legal_domain_uid,
        solution_uid = d.solution_uid,
        office_uid = d.office_uid,
        procedure_uid = d.procedure_uid,
        docket_numbers = d.docket_numbers,
        ecli = d.ecli,
        date_lecture = d.date_lecture,
        deleted_at = d.deleted_at
      FROM decisions d
     WHERE d.id = ANY(p_ids) AND t.decision_id = d.id
       AND (t.search_title, t.jurisdiction_type, t.jurisdiction_code, t.seat_paths, t.legal_instruments,
            t.legal_article_composite, t.publication_codes, t.legal_domain_uid, t.solution_uid, t.office_uid,
            t.procedure_uid, t.docket_numbers, t.ecli, t.date_lecture, t.deleted_at)
           IS DISTINCT FROM
           (d.search_title, d.jurisdiction_type, d.jurisdiction_code, d.seat_paths, d.legal_instruments,
            d.legal_article_composite, d.publication_codes, d.legal_domain_uid, d.solution_uid, d.office_uid,
            d.procedure_uid, d.docket_numbers, d.ecli, d.date_lecture, d.deleted_at);
    FOR v_id, v_text IN
        SELECT t.decision_id, t.full_text FROM decision_text t
         WHERE t.decision_id = ANY(p_ids)
           AND (SELECT count(*) FROM (SELECT 1 FROM decision_text x WHERE x.decision_id = t.decision_id LIMIT 2) n) = 1
    LOOP
        PERFORM decision_text_put(v_id, v_text);
    END LOOP;
END $$;

ALTER TABLE decisions DROP COLUMN full_text, DROP COLUMN summary, DROP COLUMN summary_engine,
    DROP COLUMN summary_prompt_version;

CREATE CONSTRAINT TRIGGER decisions_mirror_text
    AFTER INSERT OR UPDATE OF search_title, lang,
        jurisdiction_type, jurisdiction_code, seat_paths, legal_instruments, legal_article_composite,
        publication_codes, legal_domain_uid, solution_uid, office_uid, procedure_uid, docket_numbers,
        ecli, date_lecture, deleted_at
    ON decisions DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION decision_text_mirror_trigger();
