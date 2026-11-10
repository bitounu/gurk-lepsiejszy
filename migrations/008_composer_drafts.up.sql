-- Unsent text belongs to its conversation and shares the encrypted database.
CREATE TABLE composer_drafts (
    channel_id BLOB PRIMARY KEY NOT NULL,
    text TEXT NOT NULL,
    cursor INTEGER NOT NULL,
    editing INTEGER,
    attachments BLOB NOT NULL
);

-- Keep active context independently of whether its composer is empty.
CREATE TABLE composer_context (
    id INTEGER PRIMARY KEY NOT NULL CHECK (id = 0),
    channel_id BLOB NOT NULL
);
