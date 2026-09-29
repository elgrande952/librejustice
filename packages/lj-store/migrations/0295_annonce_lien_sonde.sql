CREATE TABLE annonce_lien_sonde (
    moteur   text  NOT NULL,
    kind     text  NOT NULL,
    jour     date  NOT NULL,
    verdicts jsonb NOT NULL,
    exemples jsonb NOT NULL,
    PRIMARY KEY (moteur, kind, jour)
);
