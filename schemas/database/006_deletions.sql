CREATE TABLE deletions (
    leaf_index    BYTEA       NOT NULL PRIMARY KEY,
    commitment    BYTEA       NOT NULL UNIQUE
)