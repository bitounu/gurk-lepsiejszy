-- Local row keys are separate from immutable Signal protocol timestamps.
CREATE TABLE message_identity (
    channel_id BLOB NOT NULL,
    local_id INTEGER NOT NULL,
    from_id BLOB NOT NULL,
    protocol_time INTEGER NOT NULL,
    quote_snapshot BLOB,
    is_snapshot INTEGER NOT NULL DEFAULT 0 CHECK(is_snapshot IN (0,1)),
    PRIMARY KEY(channel_id, local_id),
    FOREIGN KEY(channel_id, local_id) REFERENCES messages(channel_id, arrived_at) ON DELETE CASCADE
);
INSERT INTO message_identity(channel_id,local_id,from_id,protocol_time)
SELECT channel_id,arrived_at,from_id,arrived_at FROM messages;
CREATE INDEX message_identity_time ON message_identity(channel_id,protocol_time,local_id);
CREATE UNIQUE INDEX message_identity_protocol ON message_identity(channel_id,from_id,protocol_time) WHERE is_snapshot=0;
