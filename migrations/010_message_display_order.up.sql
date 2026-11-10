-- The sequence is local metadata; Signal timestamps and references remain untouched.
CREATE TABLE message_display_order (
    display_order INTEGER PRIMARY KEY AUTOINCREMENT,
    channel_id BLOB NOT NULL,
    arrived_at INTEGER NOT NULL,
    UNIQUE (channel_id, arrived_at),
    FOREIGN KEY (channel_id, arrived_at) REFERENCES messages (channel_id, arrived_at) ON DELETE CASCADE
);
CREATE INDEX idx_message_display_order_channel ON message_display_order (channel_id, display_order);
-- Existing history retains its chronological order when opting into received ordering.
INSERT INTO message_display_order (channel_id, arrived_at)
SELECT channel_id, arrived_at FROM messages ORDER BY arrived_at, channel_id;
CREATE TRIGGER assign_message_display_order AFTER INSERT ON messages BEGIN
    INSERT OR IGNORE INTO message_display_order (channel_id, arrived_at)
    VALUES (NEW.channel_id, NEW.arrived_at);
END;
