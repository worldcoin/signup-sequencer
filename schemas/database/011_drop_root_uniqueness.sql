ALTER TABLE identities
    DROP CONSTRAINT identities_root_key;

CREATE INDEX identities_root ON identities (root);
