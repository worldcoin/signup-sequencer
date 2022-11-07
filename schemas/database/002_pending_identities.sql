CREATE TABLE pending_identities
(
    commitment     BYTEA     NOT NULL,
    group_id       BIGINT    NOT NULL,
    created_at     TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (group_id, commitment)
)
