-- Super reactions: a showier reaction, counted apart from the plain ones so
-- clients that don't know them see unchanged reaction counts.
CREATE TABLE super_reactions (
    message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    emoji_name TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS')),
    PRIMARY KEY (message_id, user_id, emoji_name)
);

CREATE INDEX idx_super_reactions_message ON super_reactions(message_id);
