CREATE OR REPLACE FUNCTION sync_chunks_legal_instruments() RETURNS TRIGGER AS $$
DECLARE
    target_decision_id BIGINT;
    new_arr TEXT[];
BEGIN
    target_decision_id := COALESCE(NEW.decision_id, OLD.decision_id);
    SELECT ARRAY_AGG(DISTINCT lc.name ORDER BY lc.name) INTO new_arr
    FROM decision_legal_references dlr
    JOIN legal_articles la ON la.id = dlr.article_id
    JOIN legal_codes    lc ON lc.id = la.code_id
    WHERE dlr.decision_id = target_decision_id;

    UPDATE decision_chunks
    SET legal_instruments = new_arr
    WHERE decision_id = target_decision_id
      AND legal_instruments IS DISTINCT FROM new_arr;
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;
