-- Profile extras (pronouns, profile theme colours) and the private notes a
-- user keeps about others.
CREATE TABLE user_profiles (
    user_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    pronouns TEXT,
    theme_primary BIGINT,
    theme_accent BIGINT
);

CREATE TABLE user_notes (
    owner_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    target_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    note TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS')),
    PRIMARY KEY (owner_id, target_id)
);
