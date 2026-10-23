-- A space's stickers (bigger than emoji, one per message) and which messages
-- carry one.
CREATE TABLE IF NOT EXISTS stickers (
    id TEXT PRIMARY KEY NOT NULL,
    space_id TEXT NOT NULL REFERENCES spaces(id) ON DELETE CASCADE,
    creator_id TEXT REFERENCES users(id) ON DELETE SET NULL,
    name TEXT NOT NULL,
    description TEXT,
    tags TEXT,
    image TEXT NOT NULL,
    format TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_stickers_space ON stickers(space_id);

CREATE TABLE IF NOT EXISTS message_stickers (
    message_id TEXT PRIMARY KEY NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    sticker_id TEXT NOT NULL
);
