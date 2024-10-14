DROP INDEX idx_unique_insertion_leaf;
DROP INDEX idx_unique_deletion_leaf;

DROP TRIGGER validate_pre_root_trigger ON identities;
DROP FUNCTION validate_pre_root();

ALTER TABLE identities DROP COLUMN pre_root;