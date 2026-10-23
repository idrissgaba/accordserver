-- Profile extras (pronouns, profile theme colours) and the private notes a
-- user keeps about others.
CREATE TABLE IF NOT EXISTS user_profiles (
    user_id TEXT PRIMARY KEY NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    pronouns TEXT,
    theme_primary INTEGER,
    theme_accent INTEGER
);

CREATE TABLE IF NOT EXISTS user_notes (
    owner_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    target_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    note TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (owner_id, target_id)
);
