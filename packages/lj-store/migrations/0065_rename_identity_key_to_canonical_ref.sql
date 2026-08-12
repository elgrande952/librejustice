-- Migration 0065 — Renomme `decisions.identity_key` → `canonical_ref` (clé
-- d'identité v2). Amende la clé `identity_key` posée par 0061
--, jugée dangereuse à l'usage (faux merges massifs).
--
-- `canonical_ref` est la **citation légale** de la décision (cour (+ ville) |
-- numéro/RG propre | date) — identité de repli quand l'ECLI manque et clé de
-- pont inter-sources. Index NON unique (les affaires sérielles partagent
-- légitimement cour + RG + date ; l'unicité est portée par
-- `ecli`/Portalis quand ils existent, pas imposée à la citation).
--
-- ORDRE D'EXÉCUTION (cutover, pas expand/contract) : l'upsert normal écrit
-- cette colonne (`insert_decision`/`write_canonical_content` référencent
-- `identity_key` jusqu'au déploiement du nouveau binaire). Appliquer cette
-- migration UNIQUEMENT après l'arrêt de tout ancien binaire et le déploiement du
-- neuf (qui écrit `canonical_ref`). Sinon un ancien binaire référencerait une
-- colonne renommée → crash de l'ingest. Le rename SQL lui-même est instantané
-- (métadonnée), pas de réécriture de table ni d'index.

ALTER TABLE decisions
    RENAME COLUMN identity_key TO canonical_ref;

ALTER INDEX idx_decisions_identity_key
    RENAME TO idx_decisions_canonical_ref;
