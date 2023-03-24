CREATE TABLE identities
(
    leaf_index BIGINT NOT NULL PRIMARY KEY,
    commitment BYTEA  NOT NULL UNIQUE
);

CREATE TABLE root_history (
    root            BYTEA       PRIMARY KEY,
    leaf_index      BIGINT      NOT NULL,
    status          VARCHAR(50) NOT NULL,
    -- When this identity + root was accepted
    pending_as_of   TIMESTAMPTZ NOT NULL,
    -- When this identity + root was mined
    mined_at        TIMESTAMPTZ,

    FOREIGN KEY(leaf_index) REFERENCES identities(leaf_index)
);
