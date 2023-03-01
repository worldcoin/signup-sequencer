CREATE TABLE root_history
(
    root          BYTEA       NOT NULL PRIMARY KEY,
    seen_at       DATETIME    NOT NULL
);