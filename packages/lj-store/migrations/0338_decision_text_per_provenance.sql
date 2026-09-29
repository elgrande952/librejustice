
CREATE FUNCTION decision_text_put(p_id BIGINT, p_source_uid TEXT, p_lang TEXT, p_text TEXT) RETURNS VOID LANGUAGE plpgsql AS $$
DECLARE
    v_original TEXT;
BEGIN
    IF (SELECT x.source_uid FROM decision_sources x
          JOIN decision_sources me ON me.source_uid = p_source_uid
         WHERE x.decision_id = p_id
           AND coalesce(x.lang, 'fra') = coalesce(me.lang, 'fra')
         ORDER BY x.source_rank DESC, x.source_uid ASC LIMIT 1) IS DISTINCT FROM p_source_uid THEN
        RETURN;
    END IF;
    SELECT d.lang INTO STRICT v_original FROM decisions d WHERE d.id = p_id;
    INSERT INTO decision_text (
        decision_id, lang, text_status, lead, source_uid, full_text, search_title,
        jurisdiction_type, jurisdiction_code, seat_paths, legal_instruments, legal_article_composite,
        publication_codes, legal_domain_uid, solution_uid, office_uid, procedure_uid, docket_numbers,
        ecli, date_lecture, deleted_at)
    SELECT d.id, p_lang, CASE WHEN p_lang = v_original THEN 'authentic' ELSE 'official' END, FALSE,
           p_source_uid, p_text, d.search_title,
           d.jurisdiction_type, d.jurisdiction_code, d.seat_paths, d.legal_instruments, d.legal_article_composite,
           d.publication_codes, d.legal_domain_uid, d.solution_uid, d.office_uid, d.procedure_uid, d.docket_numbers,
           d.ecli, d.date_lecture, d.deleted_at
      FROM decisions d WHERE d.id = p_id
    ON CONFLICT (decision_id, lang) DO UPDATE SET
        text_status = EXCLUDED.text_status,
        source_uid = EXCLUDED.source_uid,
        full_text = EXCLUDED.full_text
     WHERE (decision_text.text_status, decision_text.source_uid, decision_text.full_text)
           IS DISTINCT FROM (EXCLUDED.text_status, EXCLUDED.source_uid, EXCLUDED.full_text);
    UPDATE decision_text t SET lead = (t.id = l.id)
      FROM (SELECT x.id FROM decision_text x LEFT JOIN decision_sources s ON s.source_uid = x.source_uid
             WHERE x.decision_id = p_id
             ORDER BY s.source_rank DESC NULLS LAST, (s.lang = 'fra') IS TRUE DESC, (s.lang = 'eng') IS TRUE DESC,
                      x.source_uid ASC
             LIMIT 1) l
     WHERE t.decision_id = p_id AND t.lead IS DISTINCT FROM (t.id = l.id);
END $$;

CREATE OR REPLACE FUNCTION decision_text_mirror(p_ids BIGINT[]) RETURNS VOID LANGUAGE sql AS $$
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
$$;
