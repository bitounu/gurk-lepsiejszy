-- Local visibility overrides are independent of synchronized conversation data.
CREATE TABLE hidden_channels (
    channel_id BLOB PRIMARY KEY NOT NULL
);
