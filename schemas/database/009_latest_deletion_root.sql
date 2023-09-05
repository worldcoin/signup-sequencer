CREATE TABLE latest_deletion_root (
    deletion_root   BYTEA  NOT NULL UNIQUE
    root_expiry     TIMESTAMPTZ,
)