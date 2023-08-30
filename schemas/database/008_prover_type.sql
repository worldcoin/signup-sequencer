-- Create ENUM for prover type
CREATE TYPE prover_enum AS ENUM('Insertion', 'Deletion');

-- Add new column with the enum
ALTER TABLE provers ADD COLUMN prover_type prover_enum;

-- Update the new column, setting all existing provers as insertions
UPDATE provers SET prover_type = 'Insertion' WHERE prover_type IS NULL;

-- Make the column NOT NULL
ALTER TABLE provers ALTER COLUMN prover_type SET NOT NULL;
