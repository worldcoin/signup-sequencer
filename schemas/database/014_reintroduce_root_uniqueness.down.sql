-- Same as migration 011_drop_root_uniqueness.sql

DROP INDEX identities_root_key;

CREATE INDEX identities_root ON identities (root);
