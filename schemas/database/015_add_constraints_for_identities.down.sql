DROP UNIQUE INDEX idx_unique_insertion_leaf;
DROP UNIQUE INDEX idx_unique_deletion_leaf;

DROP TRIGGER validate_pre_root_trigger;
DROP FUNCTION validate_pre_root();

ALTER TABLE identities DROP COLUMN pre_root;