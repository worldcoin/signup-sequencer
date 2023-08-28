CREATE TABLE unprocessed_identities (
    commitment    BYTEA       NOT NULL UNIQUE,
    status        VARCHAR(50) NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL,
    processed_at  TIMESTAMPTZ,
    error_message TEXT,
    eligibility_timestamp TIMESTAMPTZ
)