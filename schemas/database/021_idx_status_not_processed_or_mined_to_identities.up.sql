-- no-transaction
CREATE INDEX CONCURRENTLY idx_status_not_processed_or_mined ON identities(status) WHERE status <> 'processed' AND status <> 'mined';
