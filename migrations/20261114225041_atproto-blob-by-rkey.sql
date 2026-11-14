DROP TABLE IF EXISTS atproto_blobs;

CREATE TABLE IF NOT EXISTS atproto_blobs (
    rkey TEXT PRIMARY KEY,
    content_hash BLOB NOT NULL,
    media_type TEXT NOT NULL,
    cid TEXT NOT NULL,
    size INTEGER NOT NULL
);
