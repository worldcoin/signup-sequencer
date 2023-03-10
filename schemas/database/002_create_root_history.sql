-- SQL requires a composite unique key for the foreign key above to work
CREATE UNIQUE INDEX commitment_and_index on identities (commitment, leaf_index);

CREATE TABLE root_history
(
    root           BYTEA       NOT NULL PRIMARY KEY,
    last_identity  BYTEA       NOT NULL,
    last_index     BIGINT      NOT NULL,
    status         VARCHAR(50) NOT NULL,
    created_at     TIMESTAMP    NOT NULL,
    mined_at       TIMESTAMP,
    FOREIGN KEY(last_identity, last_index) REFERENCES identities(commitment, leaf_index)
);
