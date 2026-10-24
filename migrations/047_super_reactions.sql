-- Super reactions: a showier reaction, counted apart from the plain ones so
-- clients that don't know them see unchanged reaction counts.
CREATE TABLE IF NOT EXISTS super_reactions (
    message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    emoji_name TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (message_id, user_id, emoji_name)
);

CREATE INDEX IF NOT EXISTS idx_super_reactions_message ON super_reactions(message_id);
