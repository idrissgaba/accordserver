-- Polls: a question with up to ten answers attached to a message, and the
-- members' votes. A poll closes at expires_at (or earlier, when ended).
CREATE TABLE IF NOT EXISTS polls (
    message_id TEXT PRIMARY KEY NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    question TEXT NOT NULL,
    allow_multiselect INTEGER NOT NULL DEFAULT 0,
    expires_at TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS poll_answers (
    message_id TEXT NOT NULL REFERENCES polls(message_id) ON DELETE CASCADE,
    answer_id INTEGER NOT NULL,
    text TEXT NOT NULL,
    emoji TEXT,
    PRIMARY KEY (message_id, answer_id)
);

CREATE TABLE IF NOT EXISTS poll_votes (
    message_id TEXT NOT NULL REFERENCES polls(message_id) ON DELETE CASCADE,
    answer_id INTEGER NOT NULL,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (message_id, answer_id, user_id)
);

CREATE INDEX IF NOT EXISTS idx_poll_votes_user ON poll_votes(user_id);
