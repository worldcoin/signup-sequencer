CREATE UNIQUE INDEX identities_unique_commitment on identities(commitment) WHERE commitment != E'\\x0000000000000000000000000000000000000000000000000000000000000000';