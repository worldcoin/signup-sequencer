CREATE TABLE tree (
    idx         INT8      PRIMARY KEY    NOT NULL,
    hash        BYTEA                    NOT NULL
);

CREATE TABLE transactions (
    nonce       INT8      PRIMARY KEY    NOT NULL,
    raw         BYTEA                    NOT NULL,
    hash        BYTEA                        NULL
);

CREATE TABLE pending_commitments (
    idx         INT8      PRIMARY KEY    NOT NULL,
    commitment  BYTEA                    NOT NULL,
    tx_hash     BYTEA                    NULL,
    tx_time     TIMESTAMP                NULL
);
