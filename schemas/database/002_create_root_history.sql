CREATE TABLE root_history
(
    root        BYTEA      NOT NULL PRIMARY KEY,
    status      VARCHAR(50) NOT NULL,
    updated_at  DATETIME
);