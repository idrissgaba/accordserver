-- Incoming webhooks: a secret URL that posts into one channel. Each webhook
-- posts as its own bot user (users.bot), so its messages render like any
-- other author's.
CREATE TABLE IF NOT EXISTS webhooks (
    id TEXT PRIMARY KEY NOT NULL,
    space_id TEXT NOT NULL REFERENCES spaces(id) ON DELETE CASCADE,
    channel_id TEXT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES users(id),
    creator_id TEXT REFERENCES users(id) ON DELETE SET NULL,
    name TEXT NOT NULL,
    token TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_webhooks_channel ON webhooks(channel_id);
CREATE INDEX IF NOT EXISTS idx_webhooks_space ON webhooks(space_id);

-- The display name a webhook call asked for (`username`), per message.
CREATE TABLE IF NOT EXISTS webhook_messages (
    message_id TEXT PRIMARY KEY NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    webhook_id TEXT NOT NULL,
    username TEXT
);
