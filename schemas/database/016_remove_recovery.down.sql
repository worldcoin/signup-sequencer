CREATE TABLE recoveries (
    existing_commitment    BYTEA       NOT NULL UNIQUE,
    new_commitment         BYTEA       NOT NULL UNIQUE
);

ALTER TABLE unprocessed_identities
    ADD COLUMN eligibility TIMESTAMPTZ,
    ADD COLUMN status VARCHAR(50) NOT NULL,
    ADD COLUMN processed_at TIMESTAMPTZ,
    ADD COLUMN error_message TEXT;

ALTER TABLE unprocessed_identities
    DROP CONSTRAINT unique_commitment;
