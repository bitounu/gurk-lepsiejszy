-- Stored inside the same encrypted database as the messages.
CREATE TABLE private_read_sync_outbox (
    sender BLOB NOT NULL,
    timestamp INTEGER NOT NULL CHECK (timestamp >= 0),
    PRIMARY KEY (sender, timestamp)
);
CREATE TABLE private_read_sync_confirmed (
    sender BLOB NOT NULL,
    timestamp INTEGER NOT NULL CHECK (timestamp >= 0),
    PRIMARY KEY (sender, timestamp)
);
