CREATE INDEX CONCURRENTLY idx_leaf_index ON identities(leaf_index);
CREATE INDEX CONCURRENTLY idx_status_not_mined ON identities(status) WHERE status <> 'mined';
CREATE INDEX CONCURRENTLY idx_status_not_processed_or_mined ON identities(status) WHERE status <> 'processed' AND status <> 'mined';
