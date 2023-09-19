CREATE TABLE recoveries (
    existing_commitment    BYTEA       NOT NULL UNIQUE,
    new_commitment         BYTEA       NOT NULL UNIQUE
)
