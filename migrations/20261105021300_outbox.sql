CREATE TABLE IF NOT EXISTS outbox (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    published INTEGER NOT NULL,
    value TEXT NOT NULL
);

CREATE INDEX idx_outbox_sorted ON outbox(published DESC, id DESC);