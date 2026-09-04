-- Migration 0271 : un recueil rassemble des pièces qui renumérotent chacune leurs
-- articles à partir de 1. Les articles de la pièce de tête gardent leur `num_key`
-- nue, les autres la qualifient par leur pièce. L'article porte sa pièce mais
-- jamais son rang, d'où cette colonne ; nulle, le recueil garde ses numéros nus,
-- ce qui en fait aussi le curseur de la requalification.

ALTER TABLE legal_text ADD COLUMN base_piece_uid TEXT;
