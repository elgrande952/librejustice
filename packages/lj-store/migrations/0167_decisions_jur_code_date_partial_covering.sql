-- Le catalogue /juridictions agrège 3,66 M de lignes ; avec
-- l'index plein de la 0164, les filtres deleted_at/public_id forçaient la
-- lecture du heap entier (~7 s, 1,5 M de buffers). L'index partiel porte les
-- prédicats et `id` en tiebreak : catalogue et hubs années en index-only scan,
-- pages année servies dans l'ordre de l'index.

DROP INDEX idx_decisions_jur_code_date;
CREATE INDEX idx_decisions_jur_code_date ON decisions (jurisdiction_code, date_lecture DESC, id)
  WHERE deleted_at IS NULL AND public_id IS NOT NULL;
