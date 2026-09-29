CREATE TABLE decision_quantum (
    decision_id           bigint      NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    pc_extract_version    smallint    NOT NULL,
    est_liquidation       boolean     NOT NULL,
    fait_generateur       text,
    regime                text,
    devise                text,
    bareme_capitalisation text,
    total_enonce_cents    bigint,
    total_enonce_base     text,
    atteste               boolean,
    raison_non_atteste    text,
    cases_confirmees      integer,
    corrections           jsonb,
    commentaire           text,
    lecture               jsonb,
    updated_at            timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (decision_id, pc_extract_version)
);

CREATE TABLE decision_quantum_victime (
    decision_id                bigint   NOT NULL,
    pc_extract_version         smallint NOT NULL,
    ord                        smallint NOT NULL,
    cle                        text     NOT NULL,
    role                       text     NOT NULL,
    lien                       text,
    sexe                       text,
    date_naissance             date,
    age_fait                   smallint,
    date_consolidation         date,
    age_consolidation          smallint,

    age_decision               smallint,
    taux_dfp_pct               double precision,
    partage_responsabilite_pct double precision,
    perte_de_chance_pct        double precision,
    profession                 text,

    mineur                     boolean,
    cohabitation               boolean,
    orphelin                   boolean,
    frequence_relations        text,
    postes_reduits             text[]   NOT NULL DEFAULT '{}',
    postes_reserves            text[]   NOT NULL DEFAULT '{}',
    total_alloue_cents          bigint,
    assiette_reduite_cents      bigint,
    provisions_deduites_cents   bigint,
    creance_tiers_payeurs_cents bigint,
    net_victime_cents           bigint,
    exhaustif                  boolean,
    ecart_liquidation          jsonb,
    champs_indecidables        text[],
    PRIMARY KEY (decision_id, pc_extract_version, ord),
    FOREIGN KEY (decision_id, pc_extract_version)
        REFERENCES decision_quantum (decision_id, pc_extract_version) ON DELETE CASCADE
);

CREATE TABLE decision_quantum_poste (
    decision_id              bigint   NOT NULL,
    pc_extract_version       smallint NOT NULL,
    victime_ord              smallint NOT NULL,
    ord                      smallint NOT NULL,
    poste                    text     NOT NULL,
    libelle_decision         text,
    montant_demande_cents    bigint,
    montant_offert_cents     bigint,
    montant_alloue_cents     bigint,
    part_victime_cents       bigint,
    part_tiers_payeurs_cents bigint,

    valeur_unitaire_cents    bigint,
    quantite                 double precision,
    unite                    text,
    taux_pct                 double precision,
    cotation                 double precision,
    char_start               integer,
    char_end                 integer,
    composantes              jsonb,
    PRIMARY KEY (decision_id, pc_extract_version, victime_ord, ord),
    FOREIGN KEY (decision_id, pc_extract_version, victime_ord)
        REFERENCES decision_quantum_victime (decision_id, pc_extract_version, ord) ON DELETE CASCADE
);

CREATE INDEX idx_decision_quantum_poste_version_poste
    ON decision_quantum_poste (pc_extract_version, poste);
