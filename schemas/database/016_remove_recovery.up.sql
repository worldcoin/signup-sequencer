DROP TABLE recoveries;

ALTER TABLE unprocessed_identities
    DROP COLUMN eligibility,
    DROP COLUMN status,
    DROP COLUMN processed_at,
    DROP COLUMN error_message;
