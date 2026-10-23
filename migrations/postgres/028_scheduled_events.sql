-- Scheduled events of a space (in a voice channel, or somewhere else) and the
-- members interested in them.
CREATE TABLE scheduled_events (
    id TEXT PRIMARY KEY,
    space_id TEXT NOT NULL REFERENCES spaces(id) ON DELETE CASCADE,
    channel_id TEXT REFERENCES channels(id) ON DELETE SET NULL,
    creator_id TEXT REFERENCES users(id) ON DELETE SET NULL,
    name TEXT NOT NULL,
    description TEXT,
    location TEXT,
    entity_type TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'scheduled',
    start_time TEXT NOT NULL,
    end_time TEXT,
    image TEXT,
    created_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS'))
);

CREATE INDEX idx_scheduled_events_space ON scheduled_events(space_id);
CREATE INDEX idx_scheduled_events_status ON scheduled_events(status, start_time);

CREATE TABLE scheduled_event_users (
    event_id TEXT NOT NULL REFERENCES scheduled_events(id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS')),
    PRIMARY KEY (event_id, user_id)
);
