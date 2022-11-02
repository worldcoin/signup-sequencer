CREATE TABLE log_fetches
(
    id          SERIAL PRIMARY KEY NOT NULL,
    started     TIMESTAMP          NOT NULL,
    completed   TIMESTAMP,
    first_block BIGINT             NOT NULL,
    last_block  BIGINT             NOT NULL
);

CREATE TABLE logs
(
    id                  SERIAL PRIMARY KEY NOT NULL,
    block_index         BIGINT NOT NULL,
    raw                 TEXT   NOT NULL
);
