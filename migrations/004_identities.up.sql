-- Add the new 'id' column
ALTER TABLE identities ADD COLUMN id BIGINT;

-- Populate 'id' with 'leaf_index'
UPDATE identities SET id = leaf_index;

-- Set the new 'id' column as NOT NULL
ALTER TABLE identities ALTER COLUMN id SET NOT NULL;

-- Drop the unique commitment constraint to allow for 0x00 to be inserted for deletions
ALTER TABLE identities DROP CONSTRAINT identities_commitment_key;

-- Set 'id' to be unique
ALTER TABLE identities ADD CONSTRAINT id_unique UNIQUE(id);

-- Drop the existing primary key
ALTER TABLE identities DROP CONSTRAINT identities_pkey;

-- Set the new 'id' column as the primary key
ALTER TABLE identities ADD PRIMARY KEY (id);

-- Create a new sequence manually
CREATE SEQUENCE identities_id_seq;

-- Initialize a counter based on the max 'leaf_index' value
SELECT setval('identities_id_seq', coalesce((SELECT MAX(leaf_index) FROM identities), 1));

-- Set default value of id to use the sequence
ALTER TABLE identities ALTER COLUMN id SET DEFAULT nextval('identities_id_seq');
