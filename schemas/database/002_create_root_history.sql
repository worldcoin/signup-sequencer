CREATE TABLE root_history
(
    root           BYTEA       NOT NULL PRIMARY KEY,
    identity_count BIGINT      NOT NULL,
    status         VARCHAR(50) NOT NULL,
    created_at     DATETIME    NOT NULL,
    mined_at       DATETIME
);