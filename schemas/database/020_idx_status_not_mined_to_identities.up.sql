-- no-transaction
CREATE INDEX CONCURRENTLY idx_status_not_mined ON identities(status) WHERE status <> 'mined';
