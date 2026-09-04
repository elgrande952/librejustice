-- Migration 0288. Search history filters move from `chamber` to `seat`.
--
-- `user_search_history.filters` is the serde projection of `SearchRequest`: the
-- `chamber` key has left the contract, and a chip naming it could not be
-- replayed by anything. The couple's format changed four times, and the history
-- keeps the debris: of 195 persisted values, 53 still name a room of the corpus
-- (51 rows out of 181); the others carry a genre (`MAT:`, `CHS:`, `CH:`) or a
-- bare key no decision holds any more.
--
-- The 53 become seat paths, the mapping read off the corpus on 03/09/2026. The
-- debris stays in place: converting it would mean inventing the court its value
-- does not name.

WITH mapping(old_value, path) AS (VALUES
    ('cc|SOCIALE',      'CC.SOCIALE'),
    ('cc|COMMERCIALE',  'CC.COMMERCIALE'),
    ('cc|CRIMINELLE',   'CC.CRIMINELLE'),
    ('cc|C2.CIVILE',    'CC.C2_CIVILE'),
    ('cc|C3.CIVILE',    'CC.C3_CIVILE'),
    ('ca_paris|P5.C4',  'CA.PARIS.P5.C4'),
    ('ca_paris|P5.C5',  'CA.PARIS.P5.C5'),
    ('ca_paris|P5.C9',  'CA.PARIS.P5.C9'),
    ('ca_paris|P5.C11', 'CA.PARIS.P5.C11')
), target AS (
    SELECT h.id,
           array_agg(DISTINCT m.path) FILTER (WHERE m.path IS NOT NULL) AS paths,
           array_agg(DISTINCT v.value) FILTER (WHERE m.path IS NULL) AS kept
    FROM comptes.user_search_history h
         CROSS JOIN LATERAL jsonb_array_elements_text(h.filters -> 'chamber') AS v(value)
         LEFT JOIN mapping m ON m.old_value = v.value
    WHERE h.filters ? 'chamber'
    GROUP BY h.id
)
UPDATE comptes.user_search_history h
SET filters = CASE
                  WHEN target.kept IS NULL THEN h.filters - 'chamber'
                  ELSE jsonb_set(h.filters, '{chamber}', to_jsonb(target.kept))
              END
              || jsonb_build_object('seat', to_jsonb(target.paths))
FROM target
WHERE h.id = target.id AND target.paths IS NOT NULL;
