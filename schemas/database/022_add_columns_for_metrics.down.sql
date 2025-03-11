ALTER TABLE deletions
    DROP COLUMN created_at;

ALTER TABLE identities
    DROP COLUMN received_at,
    DROP COLUMN inserted_at;
