CREATE TABLE logs
(
    block_index         BYTEA NOT NULL,
    transaction_index   BYTEA NOT NULL,
    log_index           BYTEA NOT NULL,
    raw                 TEXT   NOT NULL,
    UNIQUE (block_index, transaction_index, log_index)
);
