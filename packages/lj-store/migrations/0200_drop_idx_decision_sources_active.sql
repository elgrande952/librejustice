-- `idx_decision_sources_active` est redondant avec l'index plein sur
-- la même colonne de tête. Mesure du 07/08/2026 (pg_stat_user_indexes) :
-- 251 837 scans pour 912 tuples lus au total, contre 240 M scans et 252 M tuples
-- pour `idx_decision_sources_decision_id`. Le planificateur choisit rarement le
-- partiel et n'en tire presque rien ; le besoin « provenances actives d'une
-- décision » se sert de l'index plein, `deleted_at` filtré au heap.
--
-- 86 Mo rendus tout de suite, un index de moins à écrire à chaque insertion et
-- à chaque passage en `deleted_at`.
--
-- Sans `CONCURRENTLY` : un DROP d'index est un changement de catalogue plus un
-- unlink, et le `lock_timeout = '5s'` du chemin transactionnel borne
-- l'attente de l'ACCESS EXCLUSIVE sur cette table chaude.

DROP INDEX idx_decision_sources_active;
