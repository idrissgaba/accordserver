//! A space's welcome screen: a short description and up to five recommended
//! channels (each with a line of text and an emoji), shown to members on their
//! first visit. Managed with `manage_space`.

use axum::extract::{Path, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::Row;

use crate::db;
use crate::error::AppError;
use crate::gateway::events::GatewayBroadcast;
use crate::middleware::auth::AuthUser;
use crate::middleware::permissions::{require_membership, require_permission};
use crate::state::AppState;

const MAX_CHANNELS: usize = 5;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WelcomeChannel {
    pub channel_id: String,
    pub description: String,
    #[serde(default)]
    pub emoji: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WelcomeScreen {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub channels: Vec<WelcomeChannel>,
}

/// Trims and checks a welcome screen (the channels' existence is the caller's
/// job): a description of up to 140 characters, up to five channels without
/// repeats, each with a line of 1–50 characters and an emoji of up to 32.
pub fn checked(mut screen: WelcomeScreen) -> Result<WelcomeScreen, String> {
    screen.description = screen
        .description
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty());
    if screen
        .description
        .as_ref()
        .is_some_and(|d| d.chars().count() > 140)
    {
        return Err("the description has at most 140 characters".into());
    }
    if screen.channels.len() > MAX_CHANNELS {
        return Err(format!("at most {MAX_CHANNELS} recommended channels"));
    }
    let mut seen = std::collections::HashSet::new();
    for channel in &mut screen.channels {
        if !seen.insert(channel.channel_id.clone()) {
            return Err("a channel is recommended twice".into());
        }
        channel.description = channel.description.trim().to_string();
        if !(1..=50).contains(&channel.description.chars().count()) {
            return Err("each recommended channel needs a line of 1-50 characters".into());
        }
        channel.emoji = channel
            .emoji
            .take()
            .map(|e| e.trim().to_string())
            .filter(|e| !e.is_empty());
        if channel
            .emoji
            .as_ref()
            .is_some_and(|e| e.chars().count() > 32)
        {
            return Err("an emoji has at most 32 characters".into());
        }
    }
    Ok(screen)
}

async fn load(state: &AppState, space_id: &str) -> Result<WelcomeScreen, AppError> {
    let row = sqlx::query(&db::q(
        "SELECT enabled, description, channels FROM space_welcome_screens WHERE space_id = ?",
    ))
    .bind(space_id)
    .fetch_optional(&state.db)
    .await?;
    Ok(match row {
        Some(row) => {
            let channels: String = row.get("channels");
            WelcomeScreen {
                enabled: db::get_bool(&row, "enabled"),
                description: row.get("description"),
                channels: serde_json::from_str(&channels).unwrap_or_default(),
            }
        }
        None => WelcomeScreen::default(),
    })
}

/// GET /spaces/{space_id}/welcome-screen
pub async fn get_welcome_screen(
    state: State<AppState>,
    Path(space_id): Path<String>,
    auth: AuthUser,
) -> Result<Json<Value>, AppError> {
    require_membership(&state.db, &space_id, &auth.user_id).await?;
    let mut screen = load(&state, &space_id).await?;
    // Channels deleted since drop out.
    let mut kept = Vec::new();
    for channel in screen.channels {
        if db::channels::get_channel_row(&state.db, &channel.channel_id)
            .await
            .is_ok_and(|c| c.space_id.as_deref() == Some(space_id.as_str()))
        {
            kept.push(channel);
        }
    }
    screen.channels = kept;
    Ok(Json(serde_json::json!({ "data": screen })))
}

/// PATCH /spaces/{space_id}/welcome-screen — replaces the given fields.
pub async fn update_welcome_screen(
    state: State<AppState>,
    Path(space_id): Path<String>,
    auth: AuthUser,
    Json(body): Json<Value>,
) -> Result<Json<Value>, AppError> {
    require_permission(&state.db, &space_id, &auth, "manage_space").await?;
    let mut screen = load(&state, &space_id).await?;
    if let Some(enabled) = body.get("enabled") {
        screen.enabled = enabled
            .as_bool()
            .ok_or_else(|| AppError::BadRequest("enabled must be true or false".into()))?;
    }
    if let Some(description) = body.get("description") {
        screen.description = description.as_str().map(str::to_string);
    }
    if let Some(channels) = body.get("channels") {
        screen.channels = serde_json::from_value(channels.clone())
            .map_err(|e| AppError::BadRequest(format!("invalid channels: {e}")))?;
    }
    let screen = checked(screen).map_err(AppError::BadRequest)?;
    for channel in &screen.channels {
        let row = db::channels::get_channel_row(&state.db, &channel.channel_id)
            .await
            .map_err(|_| AppError::BadRequest("unknown channel".into()))?;
        if row.space_id.as_deref() != Some(space_id.as_str()) {
            return Err(AppError::BadRequest(
                "recommended channels must be in this space".into(),
            ));
        }
    }
    sqlx::query(&db::q(
        "INSERT INTO space_welcome_screens (space_id, enabled, description, channels) VALUES (?, ?, ?, ?) \
         ON CONFLICT (space_id) DO UPDATE SET enabled = excluded.enabled, \
         description = excluded.description, channels = excluded.channels",
    ))
    .bind(&space_id)
    .bind(if screen.enabled { 1i64 } else { 0i64 })
    .bind(&screen.description)
    .bind(serde_json::to_string(&screen.channels).unwrap_or_else(|_| "[]".into()))
    .execute(&state.db)
    .await?;
    if let Some(ref tx) = *state.gateway_tx.read().await {
        let _ = tx.send(GatewayBroadcast {
            space_id: Some(space_id.clone()),
            target_user_ids: None,
            event: serde_json::json!({
                "op": 0,
                "type": "welcome_screen.update",
                "data": { "space_id": space_id, "welcome_screen": screen },
            }),
            intent: "spaces".to_string(),
            required_permission: None,
        });
    }
    Ok(Json(serde_json::json!({ "data": screen })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(id: &str, line: &str) -> WelcomeChannel {
        WelcomeChannel {
            channel_id: id.into(),
            description: line.into(),
            emoji: Some(" 👋 ".into()),
        }
    }

    #[test]
    fn welcome_screens_are_checked() {
        let ok = checked(WelcomeScreen {
            enabled: true,
            description: Some("  Hi  ".into()),
            channels: vec![channel("a", " Say hello ")],
        })
        .unwrap();
        assert_eq!(ok.description.as_deref(), Some("Hi"));
        assert_eq!(ok.channels[0].description, "Say hello");
        assert_eq!(ok.channels[0].emoji.as_deref(), Some("👋"));
        let twice = WelcomeScreen {
            channels: vec![channel("a", "x"), channel("a", "y")],
            ..Default::default()
        };
        assert!(checked(twice).is_err());
        let too_many = WelcomeScreen {
            channels: (0..6).map(|i| channel(&i.to_string(), "x")).collect(),
            ..Default::default()
        };
        assert!(checked(too_many).is_err());
        let blank = WelcomeScreen {
            channels: vec![channel("a", " ")],
            ..Default::default()
        };
        assert!(checked(blank).is_err());
    }
}
