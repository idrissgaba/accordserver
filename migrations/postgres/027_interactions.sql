-- Slash commands that applications (bots) register, the interactions members
-- start with them (or with a message's buttons and menus), and what a bot
-- attached to its messages.
CREATE TABLE application_commands (
    id TEXT PRIMARY KEY,
    application_id TEXT NOT NULL REFERENCES applications(id) ON DELETE CASCADE,
    space_id TEXT REFERENCES spaces(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    description TEXT NOT NULL,
    options TEXT NOT NULL DEFAULT '[]',
    created_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS'))
);

CREATE UNIQUE INDEX idx_application_commands_name
    ON application_commands(application_id, COALESCE(space_id, ''), name);

CREATE TABLE interactions (
    id TEXT PRIMARY KEY,
    token TEXT NOT NULL,
    application_id TEXT NOT NULL REFERENCES applications(id) ON DELETE CASCADE,
    type TEXT NOT NULL,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    space_id TEXT NOT NULL,
    channel_id TEXT NOT NULL,
    name TEXT,
    source_message_id TEXT,
    response_message_id TEXT,
    ephemeral_response TEXT,
    responded BIGINT NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL
);

CREATE INDEX idx_interactions_created ON interactions(created_at);

CREATE TABLE message_components (
    message_id TEXT PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
    components TEXT NOT NULL
);

CREATE TABLE message_interactions (
    message_id TEXT PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
    interaction_id TEXT NOT NULL,
    type TEXT NOT NULL,
    name TEXT,
    user_id TEXT NOT NULL
);
