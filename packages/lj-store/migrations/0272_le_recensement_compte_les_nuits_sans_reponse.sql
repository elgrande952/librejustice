-- Nuits consécutives où l'hôte recensé n'a rien rendu au moteur qui l'énumère.
-- Remis à zéro dès qu'il répond.
ALTER TABLE profil_acheteur
  ADD COLUMN echecs_consecutifs integer NOT NULL DEFAULT 0;
