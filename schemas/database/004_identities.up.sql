-- Add the new 'id' column
ALTER TABLE identities ADD COLUMN id BIGINT;

-- Populate the new 'id' column (assuming leaf_index can be used)
UPDATE identities SET id = leaf_index;

-- Set the new 'id' column as NOT NULL
ALTER TABLE identities ALTER COLUMN id SET NOT NULL;

-- Drop the existing primary key
ALTER TABLE identities DROP CONSTRAINT identities_pkey;

-- Set the new 'id' column as the primary key
ALTER TABLE identities ADD PRIMARY KEY (id);