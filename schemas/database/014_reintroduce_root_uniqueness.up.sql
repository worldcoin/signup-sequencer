DROP INDEX identities_root;

CREATE UNIQUE INDEX identities_root_key ON identities (root);
