-- Le chemin archive du pot Annonces.
--
-- `q` est un filtre par mot : `ILIKE '%…%'` sur le titre et l'acheteur. La
-- fenêtre vivante l'exécute en Index Scan — `annonce_fond_echeance_idx`
-- (fond, echeance) borne le balayage aux marchés ouverts, et le motif n'est
-- qu'un filtre de recheck. Hors de cette fenêtre, il n'y a plus de borne : le
-- planificateur tombe en Seq Scan sur tout le fond.
--
-- Mesuré le 07/08/2026 sur 35 384 lignes : 113 ms en Seq Scan. Le backfill de
-- l'historique BOAMP porte la table à ~1,3 M de lignes, soit ~4 s par requête —
-- c'est pourquoi cet index arrive AVANT le backfill, pas après.
--
-- Deux GIN trigramme plutôt qu'un index d'expression sur la concaténation : le
-- `OR` du filtre se résout en BitmapOr sans toucher à la requête, et un motif
-- ne peut pas matcher à cheval sur la frontière des deux colonnes.
--
-- Effet mesuré à stock égal (transaction annulée) : Seq Scan 113 ms →
-- BitmapOr 1,5 ms. Coût : 10,2 Mo + 2,9 Mo pour 41 Mo de heap, soit ~480 Mo
-- projetés à 1,3 M de lignes.
--
-- Ceci ne change PAS la sémantique du filtre, qui reste une sous-chaîne sans
-- borne de mot (`q=PLU` ramène « pluvial »). Passer aux bornes de mot est une
-- décision de produit sur `q`, pas un choix d'index.

CREATE INDEX IF NOT EXISTS annonce_titre_trgm_idx
    ON annonce USING gin (titre gin_trgm_ops);

CREATE INDEX IF NOT EXISTS annonce_acteur_libelle_trgm_idx
    ON annonce USING gin (acteur_libelle gin_trgm_ops);
