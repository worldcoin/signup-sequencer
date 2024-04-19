-- Create ENUM for prover type
CREATE TYPE batch_type_enum AS ENUM('Insertion', 'Deletion');

CREATE TABLE batches
(
    next_root  BYTEA   NOT NULL UNIQUE,
    prev_root  BYTEA NOT NULL UNIQUE,
    created_at TIMESTAMPTZ NOT NULL,
    type batch_type_enum NOT NULL,
    changes BYTEA NOT NULL,

    FOREIGN KEY (prev_root) REFERENCES batches(next_root)
)

CREATE TABLE transactions
(
    transaction_id VARCHAR(256) NOT NULL UNIQUE,
    batch_next_root BYTEA NOT NULL UNIQUE,

    FOREIGN KEY (batch_next_root) REFERENCES batches(next_root)
)