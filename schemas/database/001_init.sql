CREATE TABLE identities (
    leaf_index    BYTEA       NOT NULL PRIMARY KEY,
    commitment    BYTEA       NOT NULL UNIQUE,
    root          BYTEA       NOT NULL UNIQUE,
    status        VARCHAR(50) NOT NULL,
    -- When this identity + root was accepted
    pending_as_of TIMESTAMPTZ NOT NULL,
    -- When this identity + root was mined
    mined_at      TIMESTAMPTZ
);
