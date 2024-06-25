-- Create ENUM for prover type
CREATE TABLE batches
(
    id         BIGSERIAL UNIQUE PRIMARY KEY,
    next_root  BYTEA       NOT NULL UNIQUE,
    prev_root  BYTEA UNIQUE,
    created_at TIMESTAMPTZ NOT NULL,
    batch_type VARCHAR(50) NOT NULL,
    data       JSON        NOT NULL,

    FOREIGN KEY (prev_root) REFERENCES batches (next_root) ON DELETE CASCADE
);

CREATE INDEX idx_batches_prev_root ON batches (prev_root);
CREATE UNIQUE INDEX i_single_null_prev_root ON batches ((batches.prev_root IS NULL)) WHERE batches.prev_root IS NULL;

CREATE TABLE transactions
(
    transaction_id  VARCHAR(256) NOT NULL UNIQUE PRIMARY KEY,
    batch_next_root BYTEA        NOT NULL UNIQUE,
    created_at      TIMESTAMPTZ  NOT NULL,

    FOREIGN KEY (batch_next_root) REFERENCES batches (next_root) ON DELETE CASCADE
);
