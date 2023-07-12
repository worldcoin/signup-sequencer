UPDATE identities
SET commitment = 'finalized'
WHERE commitment = 'mined';
