CREATE TABLE recoveries (
    existing_commitment    BYTEA       NOT NULL UNIQUE,
    new_commitment         BYTEA       NOT NULL UNIQUE
);

-- It may look strange but this way we restore the order of columns. The order is important
-- as we use 'SELECT * FROM' in application code and then get data based on that order.
ALTER TABLE unprocessed_identities
  RENAME COLUMN created_at TO created_at_old;

ALTER TABLE unprocessed_identities
    ADD COLUMN status VARCHAR(50),
    ADD COLUMN created_at TIMESTAMPTZ,
    ADD COLUMN processed_at TIMESTAMPTZ,
    ADD COLUMN error_message TEXT,
    ADD COLUMN eligibility TIMESTAMPTZ;

UPDATE unprocessed_identities SET created_at = created_at_old;
UPDATE unprocessed_identities SET status = 'new', eligibility = CURRENT_TIMESTAMP WHERE status IS NULL;

ALTER TABLE unprocessed_identities
    ALTER COLUMN created_at SET NOT NULL;
ALTER TABLE unprocessed_identities
    ALTER COLUMN status SET NOT NULL;
ALTER TABLE unprocessed_identities
    DROP COLUMN created_at_old;
