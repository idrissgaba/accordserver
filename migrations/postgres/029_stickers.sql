-- A space's stickers (bigger than emoji, one per message) and which messages
-- carry one.
CREATE TABLE stickers (
    id TEXT PRIMARY KEY,
    space_id TEXT NOT NULL REFERENCES spaces(id) ON DELETE CASCADE,
    creator_id TEXT REFERENCES users(id) ON DELETE SET NULL,
    name TEXT NOT NULL,
    description TEXT,
    tags TEXT,
    image TEXT NOT NULL,
    format TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS'))
);

CREATE INDEX idx_stickers_space ON stickers(space_id);

CREATE TABLE message_stickers (
    message_id TEXT PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
    sticker_id TEXT NOT NULL
);
