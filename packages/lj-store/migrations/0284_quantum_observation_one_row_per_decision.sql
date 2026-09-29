ALTER TABLE quantum_observation
    DROP CONSTRAINT quantum_observation_decision_id_pc_extract_version_fkey,
    DROP CONSTRAINT quantum_observation_pkey,
    ADD PRIMARY KEY (decision_id, victime_ord, poste_ord);

DROP INDEX idx_quantum_observation_poste;
CREATE INDEX idx_quantum_observation_poste ON quantum_observation (poste, axe);

ALTER TABLE decision_quantum
    DROP CONSTRAINT decision_quantum_pkey,
    ADD PRIMARY KEY (decision_id),
    DROP COLUMN verdict;

ALTER TABLE quantum_observation
    ADD FOREIGN KEY (decision_id) REFERENCES decision_quantum (decision_id) ON DELETE CASCADE;
