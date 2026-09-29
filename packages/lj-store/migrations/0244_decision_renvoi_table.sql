CREATE TABLE decision_renvoi (
    decision_id        int8 NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    renvoi_no          int2 NOT NULL,
    target_ref         text NOT NULL,
    target_date        date,
    nom                text,
    target_decision_id int8 REFERENCES decisions(id) ON DELETE SET NULL,
    PRIMARY KEY (decision_id, renvoi_no, target_ref)
);

CREATE INDEX decision_renvoi_cible_idx
    ON decision_renvoi (target_decision_id) WHERE target_decision_id IS NOT NULL;
