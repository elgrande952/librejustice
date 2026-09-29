ALTER TABLE quantum_observation
    DROP CONSTRAINT quantum_observation_pkey,
    ADD COLUMN composante_ord smallint NOT NULL DEFAULT -1,
    ADD COLUMN tranche_ord    smallint NOT NULL DEFAULT -1,
    ADD COLUMN nature_aide    text,
    ADD COLUMN mode_emploi    text,
    ADD COLUMN specialisee    boolean,
    ADD COLUMN nuit           boolean,
    ADD PRIMARY KEY (decision_id, victime_ord, poste_ord, composante_ord, tranche_ord);
