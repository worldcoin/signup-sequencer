CREATE TABLE latest_deletion_root (
    Lock char(1)                NOT NULL DEFAULT 'X',
    deletion_timestamp          TIMESTAMPTZ,
    constraint PK_T1            PRIMARY KEY (Lock),
    constraint CK_T1_Locked     CHECK (Lock='X')
)