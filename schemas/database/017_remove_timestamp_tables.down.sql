CREATE TABLE latest_insertion_timestamp (
    Lock char(1)                NOT NULL DEFAULT 'X',
    insertion_timestamp         TIMESTAMPTZ,
    constraint PK_T2            PRIMARY KEY (Lock),
    constraint CK_T2_Locked     CHECK (Lock='X')
);

CREATE TABLE latest_deletion_root (
    Lock char(1)                NOT NULL DEFAULT 'X',
    deletion_timestamp          TIMESTAMPTZ,
    constraint PK_T1            PRIMARY KEY (Lock),
    constraint CK_T1_Locked     CHECK (Lock='X')
);
