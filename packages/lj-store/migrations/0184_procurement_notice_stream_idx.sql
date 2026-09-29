DROP INDEX annonce_ouverte_echeance_idx;

CREATE INDEX annonce_fond_echeance_idx ON annonce (fond, echeance);
