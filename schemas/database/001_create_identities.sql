CREATE TABLE identities
(
    commitment BYTEA       NOT NULL UNIQUE,
    leaf_index BIGINT      NOT NULL PRIMARY KEY,
    status     VARCHAR(50) NOT NULL
);
CREATE UNIQUE INDEX commitments on identities (commitment);