ALTER TABLE pending_identities ADD COLUMN status VARCHAR(50) NOT NULL DEFAULT 'pending';
UPDATE pending_identities SET status = CASE WHEN mined_in_block IS NULL THEN 'pending' ELSE 'mined' END;
ALTER TABLE pending_identities DROP COLUMN mined_in_block;
