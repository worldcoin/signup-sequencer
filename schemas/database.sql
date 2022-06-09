

CREATE TABLE tree (
    index       INT8      PRIMARY KEY    NOT NULL,
    hash        BYTEA                    NOT NULL,
);

CREATE TABLE transactions (
    nonce       INT8      PRIMARY KEY    NOT NULL,
    raw         BYTEA                    NOT NULL,
    hash        BYTEA                        NULL,
);
