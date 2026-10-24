//! Scheduled events: what they look like on the wire and how input is checked.

use serde::{Deserialize, Serialize};

/// Where an event takes place.
pub const ENTITY_VOICE: &str = "voice";
pub const ENTITY_EXTERNAL: &str = "external";

/// Lifecycle: `scheduled` → `active` → `completed`, or `canceled` before it
/// starts.
pub const STATUS_SCHEDULED: &str = "scheduled";
pub const STATUS_ACTIVE: &str = "active";
pub const STATUS_COMPLETED: &str = "completed";
pub const STATUS_CANCELED: &str = "canceled";

#[derive(Debug, Clone, Serialize)]
pub struct ScheduledEvent {
    pub id: String,
    pub space_id: String,
    pub channel_id: Option<String>,
    pub creator_id: Option<String>,
    pub name: String,
    pub description: Option<String>,
    pub location: Option<String>,
    pub entity_type: String,
    pub status: String,
    pub start_time: String,
    pub end_time: Option<String>,
    pub image: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateEvent {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub entity_type: String,
    #[serde(default)]
    pub channel_id: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
    pub start_time: String,
    #[serde(default)]
    pub end_time: Option<String>,
    /// Cover image as a `data:` URI.
    #[serde(default)]
    pub image: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct UpdateEvent {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub entity_type: Option<String>,
    #[serde(default)]
    pub channel_id: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub start_time: Option<String>,
    #[serde(default)]
    pub end_time: Option<String>,
    /// `active` (start now), `completed` (end) or `canceled`.
    #[serde(default)]
    pub status: Option<String>,
    /// Cover image as a `data:` URI, or `""` to remove it.
    #[serde(default)]
    pub image: Option<String>,
}

/// Parses an RFC 3339 time into the server's canonical UTC form.
pub fn canonical_time(raw: &str) -> Result<String, String> {
    chrono::DateTime::parse_from_rfc3339(raw.trim())
        .map(|t| {
            t.with_timezone(&chrono::Utc)
                .format("%Y-%m-%dT%H:%M:%S+00:00")
                .to_string()
        })
        .map_err(|_| format!("'{raw}' is not an RFC 3339 time"))
}

pub fn parse_time(canonical: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(canonical)
        .ok()
        .map(|t| t.with_timezone(&chrono::Utc))
}

/// The fields of an event once checked, whatever the request was.
#[derive(Debug, Clone, PartialEq)]
pub struct EventFields {
    pub name: String,
    pub description: Option<String>,
    pub entity_type: String,
    pub channel_id: Option<String>,
    pub location: Option<String>,
    pub start_time: String,
    pub end_time: Option<String>,
}

impl EventFields {
    /// Checks the fields on their own (the channel's existence is the
    /// caller's job): a 1–100 character name, a description of up to 1000,
    /// a voice event in a channel, an external one somewhere with an end, and
    /// an end after the start.
    pub fn validate(mut self) -> Result<Self, String> {
        self.name = self.name.trim().to_string();
        if !(1..=100).contains(&self.name.chars().count()) {
            return Err("an event needs a name of 1-100 characters".into());
        }
        self.description = self
            .description
            .map(|d| d.trim().to_string())
            .filter(|d| !d.is_empty());
        if self
            .description
            .as_ref()
            .is_some_and(|d| d.chars().count() > 1000)
        {
            return Err("an event description has at most 1000 characters".into());
        }
        match self.entity_type.as_str() {
            ENTITY_VOICE => {
                if self.channel_id.as_deref().is_none_or(str::is_empty) {
                    return Err("a voice event needs a voice channel".into());
                }
                self.location = None;
            }
            ENTITY_EXTERNAL => {
                self.location = self
                    .location
                    .map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty());
                let Some(ref location) = self.location else {
                    return Err("an event somewhere else needs a location".into());
                };
                if location.chars().count() > 100 {
                    return Err("a location has at most 100 characters".into());
                }
                if self.end_time.is_none() {
                    return Err("an event somewhere else needs an end time".into());
                }
                self.channel_id = None;
            }
            _ => return Err("entity_type must be voice or external".into()),
        }
        self.start_time = canonical_time(&self.start_time)?;
        if let Some(ref end) = self.end_time {
            let end = canonical_time(end)?;
            if parse_time(&end) <= parse_time(&self.start_time) {
                return Err("an event must end after it starts".into());
            }
            self.end_time = Some(end);
        }
        Ok(self)
    }
}

/// Whether moving from `from` to `to` is a status change people can make.
pub fn transition_allowed(from: &str, to: &str) -> bool {
    matches!(
        (from, to),
        (STATUS_SCHEDULED, STATUS_ACTIVE)
            | (STATUS_SCHEDULED, STATUS_CANCELED)
            | (STATUS_ACTIVE, STATUS_COMPLETED)
    )
}

/// How long an event without an end time stays active.
pub const OPEN_ENDED_HOURS: i64 = 12;

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(entity_type: &str) -> EventFields {
        EventFields {
            name: " Movie night ".into(),
            description: Some("  ".into()),
            entity_type: entity_type.into(),
            channel_id: Some("c".into()),
            location: Some(" Cinema ".into()),
            start_time: "2026-11-01T20:00:00+01:00".into(),
            end_time: Some("2026-11-01T23:00:00+01:00".into()),
        }
    }

    #[test]
    fn voice_events_need_a_channel() {
        let ok = fields(ENTITY_VOICE).validate().unwrap();
        assert_eq!(ok.name, "Movie night");
        assert_eq!(ok.description, None);
        assert_eq!(ok.location, None);
        assert_eq!(ok.start_time, "2026-11-01T19:00:00+00:00");
        let mut no_channel = fields(ENTITY_VOICE);
        no_channel.channel_id = None;
        assert!(no_channel.validate().is_err());
    }

    #[test]
    fn external_events_need_a_place_and_an_end() {
        let ok = fields(ENTITY_EXTERNAL).validate().unwrap();
        assert_eq!(ok.location.as_deref(), Some("Cinema"));
        assert_eq!(ok.channel_id, None);
        let mut open = fields(ENTITY_EXTERNAL);
        open.end_time = None;
        assert!(open.validate().is_err());
        let mut nowhere = fields(ENTITY_EXTERNAL);
        nowhere.location = Some(" ".into());
        assert!(nowhere.validate().is_err());
    }

    #[test]
    fn times_are_checked() {
        let mut backwards = fields(ENTITY_VOICE);
        backwards.end_time = Some("2026-11-01T18:00:00+00:00".into());
        assert!(backwards.validate().is_err());
        let mut garbage = fields(ENTITY_VOICE);
        garbage.start_time = "tomorrow".into();
        assert!(garbage.validate().is_err());
    }

    #[test]
    fn only_forward_transitions() {
        assert!(transition_allowed(STATUS_SCHEDULED, STATUS_ACTIVE));
        assert!(transition_allowed(STATUS_SCHEDULED, STATUS_CANCELED));
        assert!(transition_allowed(STATUS_ACTIVE, STATUS_COMPLETED));
        assert!(!transition_allowed(STATUS_ACTIVE, STATUS_CANCELED));
        assert!(!transition_allowed(STATUS_COMPLETED, STATUS_ACTIVE));
        assert!(!transition_allowed(STATUS_SCHEDULED, STATUS_COMPLETED));
    }
}
