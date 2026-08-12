-- La résolution des parties at-write (passes scopées) sonde le GIN
-- lj_fold_all(denominations) une fois par clé pendante. Avec fastupdate
-- (défaut), chaque écriture d'entity (reload mensuel des registres, updates
-- non-HOT de decision_count) empile ses entrées dans la pending list GIN,
-- que CHAQUE lookup rescanne linéairement (mesuré prod 2026-07-26 :
-- 59 958 tuples / 240 pages → ~8 ms par sonde au lieu de <1 ms). L'index
-- est write-heavy en rafale mensuelle mais lookup-critique au quotidien :
-- insertion directe ; le reload mensuel s'encadre d'un toggle
-- (registries.rs). Même précédent que le GIN docket (0115). Le passage à
-- off ne vide pas la pending list existante : clean explicite.
ALTER INDEX entity_folded_all_idx SET (fastupdate = off);
SELECT gin_clean_pending_list('entity_folded_all_idx');
