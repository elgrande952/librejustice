-- Quatrième et cinquième occurrences de la pathologie GIN pending list (même
-- précédent que 0115 docket, 0170 entity, 0171 citations).
-- Les deux GIN de `decision_chunks` filtrent la recherche par article et par
-- instrument (`legal_article_composite`, `legal_instruments`), donc ils sont
-- lookup-critiques à CHAQUE requête de recherche filtrée, alors qu'ils
-- n'accumulent d'écritures que pendant la rafale d'ingest nocturne.
-- Avec fastupdate (défaut), chaque lecteur rescanne linéairement la pending
-- list : mesuré prod 2026-07-31, 311 pages en attente sur le composite et
-- 301 pages / 16 702 tuples sur les instruments. Même remède, même forme :
-- insertion directe puis vidage explicite (SET n'évacue pas la liste existante).
ALTER INDEX idx_chunks_legal_article_composite SET (fastupdate = off);
SELECT gin_clean_pending_list('idx_chunks_legal_article_composite');

ALTER INDEX idx_chunks_legal_instruments SET (fastupdate = off);
SELECT gin_clean_pending_list('idx_chunks_legal_instruments');
