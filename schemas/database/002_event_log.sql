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
    block_index         BIGINT NOT NULL,
    transaction_index   BIGINT NOT NULL,
    log_index           BIGINT NOT NULL,
    fetch_id            INT    NOT NULL REFERENCES log_fetches (id),
    group_id            BYTEA  NOT NULL,
    identity_commitment BYTEA  NOT NULL,
    root                BYTEA  NOT NULL,
    UNIQUE (block_index, transaction_index, log_index)
);
