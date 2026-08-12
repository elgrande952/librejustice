-- Retrait d'alias_of (posé en 0157) : la fusion des manifestations d'un même
-- instrument passe par le collapse d'identité.
ALTER TABLE legal_text DROP COLUMN alias_of;
