-- Migration 0243 : référentiel dérivé des chambres, sur le modèle de
-- `jurisdiction`. La clé est locale à sa cour et vaut soit une position
-- (`CH:2`), soit le qualificatif qui nomme la chambre à défaut de position
-- (`MAT:SOCIALE`). Le libellé se compose en Rust, table remplie par
-- `lj-ingest resync-chambers`.

CREATE TABLE chamber (
  jurisdiction_code TEXT NOT NULL REFERENCES jurisdiction(code),
  chamber_key       TEXT NOT NULL,
  label             TEXT NOT NULL,
  PRIMARY KEY (jurisdiction_code, chamber_key)
);
