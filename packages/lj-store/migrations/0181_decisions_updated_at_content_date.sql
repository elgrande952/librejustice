UPDATE decisions
SET updated_at = date_lecture::timestamptz
WHERE date_lecture IS NOT NULL
  AND updated_at::date <> date_lecture;
