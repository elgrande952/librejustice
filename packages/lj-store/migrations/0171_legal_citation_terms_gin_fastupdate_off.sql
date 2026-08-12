-- Les pages `/texte/{code}/{num}` sondent le GIN `lj_cit_terms(spans)` deux fois
-- par rendu — décisions citantes (`law_decisions_citing`) et articles co-cités
-- (`law_co_cited_articles`). Avec fastupdate (défaut), chaque écriture de
-- `legal_citation` (dérivation at-write des citations à l'ingest quotidien)
-- empile ses entrées dans la pending list, que CHAQUE lookup rescanne
-- linéairement (mesuré prod 2026-07-31 : 4 237 tuples / 235 pages → une sonde
-- sur un terme absent lit 239 buffers en 10,4 ms au lieu de 4 buffers en
-- 0,14 ms). L'index est write-heavy en rafale nocturne mais lookup-critique à
-- chaque page d'article : insertion directe. Troisième occurrence de la même
-- pathologie, même précédent que le GIN docket (0115) et le GIN entity
-- (0170, même doctrine). Le passage à off ne vide pas la pending list
-- existante : clean explicite.
ALTER INDEX legal_citation_terms_idx SET (fastupdate = off);
SELECT gin_clean_pending_list('legal_citation_terms_idx');
