-- A space's welcome screen: a description and up to five recommended
-- channels, shown to members on their first visit.
CREATE TABLE space_welcome_screens (
    space_id TEXT PRIMARY KEY REFERENCES spaces(id) ON DELETE CASCADE,
    enabled BIGINT NOT NULL DEFAULT 0,
    description TEXT,
    channels TEXT NOT NULL DEFAULT '[]'
);
