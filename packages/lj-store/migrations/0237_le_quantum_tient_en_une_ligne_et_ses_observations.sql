-- Migration 0237 : le quantum tient en une ligne, ses observations en une table.
--
-- `quantum_observation` est une projection reconstructible depuis le jsonb, avec
-- un écrivain unique. Une ligne par poste lu, y compris hors table : le même
-- balayage rend la distribution et sa couverture.
--
-- La ligne porte l'observation, jamais la référence à laquelle on la confronte :
-- une observation est ce que la cour a alloué, l'écart à un référentiel est une
-- lecture qui dépend de l'édition choisie.

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
    -- `grille`, `unite`, `cotation`, `categorie`. NULL quand le montant n'a
    -- aucune abscisse, et `hors_table` dit alors pourquoi.
    axe                   text,
    hors_table            text,
    montant_cents         bigint,
    -- Le tarif PLEIN, `taux_pct` portant la fraction : sans quoi une médiane
    -- mélange 6,25 et 25 euros pour le même tarif journalier.
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
    -- Dénormalisés depuis `decisions` : la distribution se ventile par cour et
    -- par année sans jointure.
    juridiction           text,
    annee                 smallint,
    PRIMARY KEY (decision_id, pc_extract_version, victime_ord, poste_ord),
    FOREIGN KEY (decision_id, pc_extract_version)
        REFERENCES decision_quantum (decision_id, pc_extract_version) ON DELETE CASCADE
);

CREATE INDEX idx_quantum_observation_poste
    ON quantum_observation (pc_extract_version, poste, axe);
