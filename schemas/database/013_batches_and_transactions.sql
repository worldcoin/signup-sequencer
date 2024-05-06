-- Create ENUM for prover type
CREATE TYPE batch_type_enum AS ENUM ('Insertion', 'Deletion');

CREATE TABLE batches
(
    next_root    BYTEA           NOT NULL UNIQUE PRIMARY KEY,
    prev_root    BYTEA UNIQUE,
    created_at   TIMESTAMPTZ     NOT NULL,
    batch_type   batch_type_enum NOT NULL,
    commitments  BYTEA[]         NOT NULL,
    leaf_indexes BIGINT[]        NOT NULL CHECK (array_length(leaf_indexes, 1) = array_length(commitments, 1)),

    FOREIGN KEY (prev_root) REFERENCES batches (next_root)
);

CREATE UNIQUE INDEX i_single_null_prev_root ON batches ((batches.prev_root IS NULL)) WHERE batches.prev_root IS NULL;

CREATE TABLE transactions
(
    transaction_id  VARCHAR(256) NOT NULL UNIQUE PRIMARY KEY,
    batch_next_root BYTEA        NOT NULL UNIQUE,
    created_at      TIMESTAMPTZ  NOT NULL,

    FOREIGN KEY (batch_next_root) REFERENCES batches (next_root)
);