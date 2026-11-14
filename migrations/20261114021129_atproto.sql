CREATE TABLE IF NOT EXISTS atproto_blobs (
    content_hash BLOB NOT NULL,
    media_type TEXT NOT NULL,
    cid TEXT NOT NULL,
    size INTEGER NOT NULL,
    PRIMARY KEY (content_hash, media_type)
);
