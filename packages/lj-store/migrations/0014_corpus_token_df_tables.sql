CREATE TABLE corpus_token_df (
    token TEXT    PRIMARY KEY,
    df    INTEGER NOT NULL
);

CREATE TABLE corpus_stats (
    key   TEXT   PRIMARY KEY,
    value BIGINT NOT NULL
);
