ALTER TABLE provers ADD COLUMN prover_type ENUM('Insertion', 'Deletion');

UPDATE provers SET prover_type = 'Insertion' WHERE prover_type IS NULL;

ALTER TABLE provers MODIFY COLUMN prover_type ENUM('Insertion', 'Deletion') NOT NULL;
