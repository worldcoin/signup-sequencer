-- delete pending_identities that are already present in the cache (i.e. they are duplicates)
DELETE FROM pending_identities WHERE commitment in (SELECT leaf from logs);