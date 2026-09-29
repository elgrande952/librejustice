ALTER TABLE jurisdiction ADD COLUMN source_code TEXT;
UPDATE jurisdiction SET source_code = code;
ALTER TABLE jurisdiction ALTER COLUMN source_code SET NOT NULL;
ALTER TABLE jurisdiction ADD CONSTRAINT jurisdiction_source_code_key UNIQUE (source_code);

ALTER TABLE decisions DROP CONSTRAINT decisions_jurisdiction_code_fkey;

UPDATE jurisdiction
SET code = lower(juridiction_type) || '_' || trim(both '_' from regexp_replace(
        translate(lower(replace(replace(city, 'œ', 'oe'), 'Œ', 'oe')),
                  'àâäáéèêëíîïóôöúùûüçÿñ', 'aaaaeeeeiiioooouuuucyn'),
        '[^a-z0-9]+', '_', 'g'))
WHERE juridiction_type IN ('TJ', 'TCOM') AND city IS NOT NULL;

UPDATE decisions d
SET jurisdiction_code = j.code
FROM jurisdiction j
WHERE d.jurisdiction_code = j.source_code
  AND j.code <> j.source_code;

ALTER TABLE decisions ADD CONSTRAINT decisions_jurisdiction_code_fkey
    FOREIGN KEY (jurisdiction_code) REFERENCES jurisdiction(code);

UPDATE user_search_history h
SET filters = jsonb_set(h.filters, '{jurisdictionCode}', (
        SELECT jsonb_agg(coalesce(j.code, x.v))
        FROM jsonb_array_elements_text(h.filters -> 'jurisdictionCode') AS x(v)
        LEFT JOIN jurisdiction j ON j.source_code = x.v AND j.code <> j.source_code))
WHERE h.filters ? 'jurisdictionCode';
