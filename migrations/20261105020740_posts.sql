CREATE TABLE IF NOT EXISTS posts (
    id TEXT PRIMARY KEY,
    valuehash BLOB NOT NULL,
    static_url TEXT NOT NULL,
    human_url TEXT
);
