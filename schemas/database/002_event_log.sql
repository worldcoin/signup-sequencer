CREATE TABLE logs
(
    block_index         BIGINT NOT NULL,
    transaction_index   INT    NOT NULL,
    log_index           INT    NOT NULL,
    raw                 TEXT   NOT NULL,
    UNIQUE (block_index, transaction_index, log_index)
);
