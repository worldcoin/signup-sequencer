CREATE TYPE prover_enum AS ENUM('Insertion', 'Deletion');

CREATE TABLE provers (
    batch_size BIGINT NOT NULL,
    url VARCHAR(1028) NOT NULL,
    timeout_s BIGINT,
    prover_type prover_enum NOT NULL
);
