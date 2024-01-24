CREATE TABLE deletions (
    leaf_index    BIGINT      NOT NULL PRIMARY KEY,
    commitment    BYTEA       NOT NULL UNIQUE
)