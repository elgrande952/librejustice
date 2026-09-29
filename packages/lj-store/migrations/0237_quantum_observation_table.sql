DROP TABLE IF EXISTS decision_quantum_poste;
DROP TABLE IF EXISTS decision_quantum_victime;
DROP TABLE IF EXISTS decision_quantum;

CREATE TABLE decision_quantum (
    decision_id        bigint      NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    pc_extract_version smallint    NOT NULL,
    est_liquidation    boolean     NOT NULL,
    atteste            boolean,
    raison_non_atteste text,
    cases_confirmees   integer,
    quantum            jsonb       NOT NULL,
    updated_at         timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (decision_id, pc_extract_version)
);

CREATE TABLE quantum_observation (
    decision_id           bigint   NOT NULL,
    pc_extract_version    smallint NOT NULL,
    victime_ord           smallint NOT NULL,
    poste_ord             smallint NOT NULL,
    poste                 text     NOT NULL,

    axe                   text,
    hors_table            text,
    montant_cents         bigint,

    valeur_unitaire_cents bigint,
    quantite              double precision,
    unite                 text,
    taux_pct              double precision,
    cotation              double precision,
    categorie             text,
    taux_dfp_pct          double precision,
    age                   smallint,
    sexe                  text,
    role                  text,
    lien                  text,

    juridiction           text,
    annee                 smallint,
    PRIMARY KEY (decision_id, pc_extract_version, victime_ord, poste_ord),
    FOREIGN KEY (decision_id, pc_extract_version)
        REFERENCES decision_quantum (decision_id, pc_extract_version) ON DELETE CASCADE
);

CREATE INDEX idx_quantum_observation_poste
    ON quantum_observation (pc_extract_version, poste, axe);
