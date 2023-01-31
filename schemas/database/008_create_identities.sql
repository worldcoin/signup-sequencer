CREATE TABLE identities
(
    commitment BYTEA       NOT NULL,
    leaf_index BIGINT      NOT NULL PRIMARY KEY,
    status     VARCHAR(50) NOT NULL DEFAULT 'pending'
);
CREATE INDEX commitments on identities (commitment);