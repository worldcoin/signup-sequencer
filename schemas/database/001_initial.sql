CREATE TABLE tree (
    idx         INT8      PRIMARY KEY    NOT NULL,
    hash        BYTEA                    NOT NULL
);

CREATE TABLE transactions (
    nonce       INT8      PRIMARY KEY    NOT NULL,
    raw         BYTEA                    NOT NULL,
    hash        BYTEA                        NULL
);

CREATE TABLE log_fetches
(
    id          SERIAL PRIMARY KEY NOT NULL,
    timestamp   TIMESTAMP          NOT NULL,
    first_block BIGINT             NOT NULL,
    last_block  BIGINT             NOT NULL,
);

CREATE TABLE logs
(
    block_index         BIGINT         NOT NULL,
    transaction_index   BIGINT         NOT NULL,
    log_index           BIGINT         NOT NULL,
    fetch_id            INT            NOT NULL REFERENCES log_fetches (id),
    group_id            NUMERIC(78, 0) NOT NULL,
    identity_commitment NUMERIC(78, 0) NOT NULL,
    root                NUMERIC(78, 0) NOT NULL,
    UNIQUE (block_index, transaction_index, log_index)
);
