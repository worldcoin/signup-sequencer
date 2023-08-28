-- Drop the new primary key constraint
ALTER TABLE identities DROP CONSTRAINT identities_pkey;

-- Restore the old primary key
ALTER TABLE identities ADD PRIMARY KEY (leaf_index);

-- Drop the new 'id' column
ALTER TABLE identities DROP COLUMN id;
