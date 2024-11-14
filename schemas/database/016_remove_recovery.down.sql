CREATE TABLE recoveries (
    existing_commitment    BYTEA       NOT NULL UNIQUE,
    new_commitment         BYTEA       NOT NULL UNIQUE
);

ALTER TABLE unprocessed_identities
    ADD COLUMN eligibility TIMESTAMPTZ,
    ADD COLUMN status VARCHAR(50),
    ADD COLUMN processed_at TIMESTAMPTZ,
    ADD COLUMN error_message TEXT;

UPDATE unprocessed_identities SET status = 'new', eligibility = CURRENT_TIMESTAMP WHERE status IS NULL;

ALTER TABLE unprocessed_identities
    ALTER COLUMN status SET NOT NULL;
