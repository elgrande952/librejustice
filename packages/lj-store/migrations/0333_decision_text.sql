CREATE TABLE decision_text (
    id                      BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    decision_id             BIGINT NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    lang                    TEXT NOT NULL CHECK (lang ~ '^[a-z]{2}$'),
    text_status             TEXT NOT NULL CHECK (text_status IN ('authentic', 'official', 'translation', 'automatic')),
    lead                    BOOLEAN NOT NULL DEFAULT TRUE,
    source_uid              TEXT,
    full_text               TEXT NOT NULL,
    search_title            TEXT,
    summary                 TEXT,
    summary_engine          SMALLINT,
    summary_prompt_version  SMALLINT,
    jurisdiction_type       TEXT NOT NULL,
    jurisdiction_code       TEXT,
    seat_paths              TEXT[],
    legal_instruments       TEXT[],
    legal_article_composite TEXT[],
    publication_codes       TEXT[] NOT NULL DEFAULT '{}',
    legal_domain_uid        TEXT,
    solution_uid            TEXT,
    office_uid              TEXT,
    procedure_uid           TEXT,
    docket_numbers          TEXT[] NOT NULL DEFAULT '{}',
    ecli                    TEXT,
    date_lecture            DATE,
    deleted_at              TIMESTAMPTZ,
    UNIQUE (decision_id, lang)
);

ALTER TABLE decisions ADD COLUMN lang TEXT NOT NULL DEFAULT 'fr';

ALTER TABLE decision_chunks ADD COLUMN lang TEXT NOT NULL DEFAULT 'fr';

CREATE FUNCTION decision_text_mirror(p_ids BIGINT[]) RETURNS VOID LANGUAGE sql AS $$
    WITH src AS (
        SELECT d.*, s.source_uid, s.source,
               CASE WHEN s.source_uid IS NULL OR s.lang IS NULL OR s.lang = 'fra' THEN 'fr'
                    WHEN s.lang = 'eng' THEN 'en' END AS text_lang
        FROM decisions d
        LEFT JOIN LATERAL (
            SELECT ds.source_uid, ds.lang, ds.source FROM decision_sources ds
            WHERE ds.decision_id = d.id
            ORDER BY ds.source_rank DESC, (ds.lang = 'fra') IS TRUE DESC, ds.source_uid ASC
            LIMIT 1
        ) s ON TRUE
        WHERE d.id = ANY(p_ids) AND d.full_text IS NOT NULL
    ), dropped AS (
        DELETE FROM decision_text t
        WHERE t.decision_id = ANY(p_ids)
          AND NOT EXISTS (SELECT 1 FROM src WHERE src.id = t.decision_id AND src.text_lang = t.lang)
    )
    INSERT INTO decision_text (
        decision_id, lang, text_status, source_uid, full_text, search_title, summary, summary_engine, summary_prompt_version,
        jurisdiction_type, jurisdiction_code, seat_paths, legal_instruments, legal_article_composite,
        publication_codes, legal_domain_uid, solution_uid, office_uid, procedure_uid, docket_numbers,
        ecli, date_lecture, deleted_at)
    SELECT id, text_lang,
           CASE WHEN source = 'cjue' AND text_lang <> lang THEN 'official' ELSE 'authentic' END,
           source_uid, full_text, search_title, summary, summary_engine, summary_prompt_version,
           jurisdiction_type, jurisdiction_code, seat_paths, legal_instruments, legal_article_composite,
           publication_codes, legal_domain_uid, solution_uid, office_uid, procedure_uid, docket_numbers,
           ecli, date_lecture, deleted_at
    FROM src
    ON CONFLICT (decision_id, lang) DO UPDATE SET
        text_status = EXCLUDED.text_status,
        source_uid = EXCLUDED.source_uid,
        full_text = CASE WHEN decision_text.full_text IS DISTINCT FROM EXCLUDED.full_text
                         THEN EXCLUDED.full_text ELSE decision_text.full_text END,
        search_title = EXCLUDED.search_title,
        summary = EXCLUDED.summary,
        summary_engine = EXCLUDED.summary_engine,
        summary_prompt_version = EXCLUDED.summary_prompt_version,
        jurisdiction_type = EXCLUDED.jurisdiction_type,
        jurisdiction_code = EXCLUDED.jurisdiction_code,
        seat_paths = EXCLUDED.seat_paths,
        legal_instruments = EXCLUDED.legal_instruments,
        legal_article_composite = EXCLUDED.legal_article_composite,
        publication_codes = EXCLUDED.publication_codes,
        legal_domain_uid = EXCLUDED.legal_domain_uid,
        solution_uid = EXCLUDED.solution_uid,
        office_uid = EXCLUDED.office_uid,
        procedure_uid = EXCLUDED.procedure_uid,
        docket_numbers = EXCLUDED.docket_numbers,
        ecli = EXCLUDED.ecli,
        date_lecture = EXCLUDED.date_lecture,
        deleted_at = EXCLUDED.deleted_at;
$$;

CREATE FUNCTION decision_text_mirror_trigger() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_TABLE_NAME = 'decisions' THEN
        PERFORM decision_text_mirror(ARRAY[NEW.id]);
    ELSIF TG_OP = 'DELETE' THEN
        PERFORM decision_text_mirror(ARRAY[OLD.decision_id]);
    ELSIF TG_OP = 'UPDATE' THEN
        PERFORM decision_text_mirror(ARRAY[OLD.decision_id, NEW.decision_id]);
    ELSE
        PERFORM decision_text_mirror(ARRAY[NEW.decision_id]);
    END IF;
    RETURN NULL;
END $$;

CREATE CONSTRAINT TRIGGER decisions_mirror_text
    AFTER INSERT OR UPDATE OF full_text, search_title, summary, summary_engine, summary_prompt_version, lang,
        jurisdiction_type, jurisdiction_code, seat_paths, legal_instruments, legal_article_composite,
        publication_codes, legal_domain_uid, solution_uid, office_uid, procedure_uid, docket_numbers,
        ecli, date_lecture, deleted_at
    ON decisions DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION decision_text_mirror_trigger();

CREATE CONSTRAINT TRIGGER decision_sources_mirror_text
    AFTER INSERT OR UPDATE OF source_uid, lang, source_rank, decision_id OR DELETE
    ON decision_sources DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION decision_text_mirror_trigger();
