-- `legal_citation` est la seule table lourde du fonds sans aucun plancher de
-- maintenance : 4,25 Go / 3,65 M lignes, elle n'est PAS dans les cinq tables du
-- `vacuum-full` hebdomadaire (decision_chunks, legal_article,
-- legal_text, legal_article_usage, decisions) et vit sous l'autovacuum par
-- défaut. Avec `autovacuum_vacuum_scale_factor = 0.2`, le seuil de déclenchement
-- est à ~730 000 tuples morts : mesuré prod 2026-07-31, 215 556 morts (5,9 %) et
-- dernier autovacuum vieux de 5 jours. C'est la cause amont de la pending list
-- GIN traitée en 0171 (le vidage de la liste se fait au vacuum) et du bloat du
-- heap le reste du temps.
--
-- Réglage par table plutôt que global : cette table n'a qu'un btree PK et le GIN
-- de termes — aucun index BM25 — donc un vacuum fréquent y est bon marché et
-- sans interaction avec les merges tantivy. Les tables de recherche gardent
-- délibérément les valeurs par défaut : elles ont déjà le plancher hebdomadaire,
-- et un autovacuum plus agressif sur leurs index BM25 demanderait sa propre
-- mesure.
ALTER TABLE legal_citation SET (
  autovacuum_vacuum_scale_factor = 0.02,
  autovacuum_analyze_scale_factor = 0.05
);
