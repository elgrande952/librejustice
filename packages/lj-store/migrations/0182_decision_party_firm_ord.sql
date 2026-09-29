ALTER TABLE decision_party ADD COLUMN firm_ord int;

CREATE INDEX decision_party_firm_ord_idx ON decision_party (decision_id, firm_ord)
    WHERE firm_ord IS NOT NULL;
