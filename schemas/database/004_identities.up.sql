-- Add the new 'id' column
ALTER TABLE identities ADD COLUMN id BIGSERIAL;

-- Set the new 'id' column as NOT NULL
ALTER TABLE identities ALTER COLUMN id SET NOT NULL;

-- Drop the unique commitment constraint to allow for 0x00 to be inserted for deletions
ALTER TABLE identities DROP CONSTRAINT identities_commitment_key;

-- Set the id to be unique
ALTER TABLE identities ADD CONSTRAINT id_unique UNIQUE(id);

-- Drop the existing primary key
ALTER TABLE identities DROP CONSTRAINT identities_pkey;

-- Set the new 'id' column as the primary key
ALTER TABLE identities ADD PRIMARY KEY (id);