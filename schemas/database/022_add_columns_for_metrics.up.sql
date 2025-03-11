ALTER TABLE deletions
    ADD COLUMN created_at TIMESTAMPTZ;

ALTER TABLE identities
    ADD COLUMN received_at TIMESTAMPTZ,
    ADD COLUMN inserted_at TIMESTAMPTZ;
