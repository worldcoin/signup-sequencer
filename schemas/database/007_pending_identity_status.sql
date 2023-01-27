ALTER TABLE pending_identities ADD COLUMN status VARCHAR(50) NOT NULL DEFAULT 'pending';
ALTER TABLE pending_identities ADD COLUMN transaction_id VARCHAR(50);
UPDATE pending_identities SET status = CASE WHEN mined_in_block IS NULL THEN 'pending' ELSE 'mined' END, transaction_id = mined_in_block;
ALTER TABLE pending_identities DROP COLUMN mined_in_block;
