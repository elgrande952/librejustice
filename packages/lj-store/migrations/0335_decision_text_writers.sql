CREATE OR REPLACE FUNCTION decision_text_put(p_id BIGINT, p_text TEXT) RETURNS VOID LANGUAGE plpgsql AS $$
DECLARE
    v_lang TEXT;
    v_status TEXT;
    v_source_uid TEXT;
BEGIN
    SELECT CASE WHEN s.source_uid IS NULL OR s.lang IS NULL OR s.lang = 'fra' THEN 'fr'
                WHEN s.lang = 'eng' THEN 'en' END,
           CASE WHEN s.source = 'cjue' AND (CASE WHEN s.lang = 'eng' THEN 'en' ELSE 'fr' END) <> d.lang
                THEN 'official' ELSE 'authentic' END,
           s.source_uid
      INTO v_lang, v_status, v_source_uid
      FROM decisions d
      LEFT JOIN LATERAL (
          SELECT ds.source_uid, ds.lang, ds.source FROM decision_sources ds
          WHERE ds.decision_id = d.id
          ORDER BY ds.source_rank DESC, (ds.lang = 'fra') IS TRUE DESC, ds.source_uid ASC
          LIMIT 1
      ) s ON TRUE
     WHERE d.id = p_id;
    IF (SELECT count(*) FROM (SELECT 1 FROM decision_text WHERE decision_id = p_id LIMIT 2) n) = 1 THEN
        UPDATE decision_text SET lang = v_lang, text_status = v_status, source_uid = v_source_uid, full_text = p_text
         WHERE decision_id = p_id
           AND (lang, text_status, source_uid, full_text) IS DISTINCT FROM (v_lang, v_status, v_source_uid, p_text);
        RETURN;
    END IF;
    INSERT INTO decision_text (
        decision_id, lang, text_status, source_uid, full_text, search_title,
        jurisdiction_type, jurisdiction_code, seat_paths, legal_instruments, legal_article_composite,
        publication_codes, legal_domain_uid, solution_uid, office_uid, procedure_uid, docket_numbers,
        ecli, date_lecture, deleted_at)
    SELECT d.id, v_lang, v_status, v_source_uid, p_text, d.search_title,
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
END $$;

CREATE OR REPLACE FUNCTION decision_text_mirror(p_ids BIGINT[]) RETURNS VOID LANGUAGE plpgsql AS $$
DECLARE
    v_id BIGINT;
    v_text TEXT;
BEGIN
    FOR v_id, v_text IN SELECT id, full_text FROM decisions WHERE id = ANY(p_ids) AND full_text IS NOT NULL LOOP
        PERFORM decision_text_put(v_id, v_text);
    END LOOP;
    UPDATE decision_text t SET
        summary = d.summary,
        summary_engine = d.summary_engine,
        summary_prompt_version = d.summary_prompt_version
      FROM decisions d
     WHERE d.id = ANY(p_ids) AND t.decision_id = d.id AND t.lead AND d.summary IS NOT NULL
       AND (t.summary, t.summary_engine, t.summary_prompt_version)
           IS DISTINCT FROM (d.summary, d.summary_engine, d.summary_prompt_version);
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
        SELECT t.decision_id, t.full_text FROM decision_text t JOIN decisions d ON d.id = t.decision_id
         WHERE t.decision_id = ANY(p_ids) AND d.full_text IS NULL
           AND (SELECT count(*) FROM (SELECT 1 FROM decision_text x WHERE x.decision_id = t.decision_id LIMIT 2) n) = 1
    LOOP
        PERFORM decision_text_put(v_id, v_text);
    END LOOP;
END $$;
