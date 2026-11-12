CREATE TABLE IF NOT EXISTS users (
    username TEXT NOT NULL,
    display_name TEXT,
    id TEXT PRIMARY KEY,
    inbox URL NOT NULL,
    shared_inbox URL,
    public_key TEXT NOT NULL,
    last_refreshed INTEGER NOT NULL,
    extra TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS local_users (
    id TEXT PRIMARY KEY,
    private_key TEXT,
    FOREIGN KEY (id) REFERENCES users (id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS followers (
    followee TEXT NOT NULL,
    follower TEXT NOT NULL,
    PRIMARY KEY (followee, follower),
    FOREIGN KEY (followee) REFERENCES local_users (id) ON DELETE CASCADE,
    FOREIGN KEY (follower) REFERENCES users (id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS blog_articles (
    id TEXT PRIMARY KEY,
    author TEXT NOT NULL,
    url TEXT NOT NULL,
    to_url TEXT NOT NULL,
    cc TEXT NOT NULL,
    extra TEXT NOT NULL,
    name TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS outbound_activities (
    id_part TEXT PRIMARY KEY,
    actor TEXT NOT NULL,
    kind TEXT NOT NULL,
    published INTEGER NOT NULL,
    object TEXT
);

CREATE INDEX outbound_activities_by_date ON outbound_activities(published);

CREATE INDEX outbound_activities_by_actor ON outbound_activities(actor);