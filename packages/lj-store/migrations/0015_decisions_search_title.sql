ALTER TABLE decisions ADD COLUMN IF NOT EXISTS search_title TEXT GENERATED ALWAYS AS (
  COALESCE(jurisdiction_name, juridiction_type)
  || CASE WHEN formation_or_chamber IS NOT NULL THEN ', ' || formation_or_chamber ELSE '' END
  || CASE WHEN date_lecture IS NOT NULL THEN
       ', ' ||
       EXTRACT(DAY FROM date_lecture)::int::text || ' ' ||
       (ARRAY['janvier','février','mars','avril','mai','juin','juillet',
              'août','septembre','octobre','novembre','décembre'])[EXTRACT(MONTH FROM date_lecture)::int] ||
       ' ' || EXTRACT(YEAR FROM date_lecture)::int::text
     ELSE '' END
  || CASE WHEN docket_numbers[1] IS NOT NULL THEN ', ' || docket_numbers[1] ELSE '' END
) STORED;

CREATE INDEX IF NOT EXISTS decisions_title_bm25 ON decisions
USING bm25 (id, search_title)
WITH (
  key_field = 'id',
  text_fields = '{"search_title": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}-]+", "ascii_folding": true}, "record": "position"}}'
);
