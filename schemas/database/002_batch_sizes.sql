CREATE TABLE provers (
    batch_size: BIGINT NOT NULL PRIMARY KEY,
    url: VARCHAR(1028) NOT NULL UNIQUE,
    timeout_s: BIGINT,
)