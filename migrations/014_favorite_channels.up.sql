-- Local favorites survive renames and remain independent of visibility.
CREATE TABLE favorite_channels (
    channel_id BLOB PRIMARY KEY NOT NULL
);
