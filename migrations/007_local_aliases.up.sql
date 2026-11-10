-- Keep local display overrides independent of names synchronized with Signal.
CREATE TABLE person_aliases (
    id BLOB PRIMARY KEY NOT NULL,
    alias TEXT NOT NULL
);

CREATE TABLE channel_aliases (
    id BLOB PRIMARY KEY NOT NULL,
    alias TEXT NOT NULL
);
