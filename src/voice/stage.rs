//! Stage channels: a voice channel with an audience. Everyone joins in the
//! audience, suppressed (enforced in LiveKit like any other loss of `speak`).
//! Members raise a hand to ask to speak; stage moderators — members with
//! `mute_members` on the channel — bring people up, send them back to the
//! audience, and may step up themselves.

use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;

use crate::db;
use crate::error::AppError;
use crate::middleware::auth::AuthUser;
use crate::middleware::permissions::require_channel_permission;
use crate::state::AppState;

/// Channel types members connect to by voice.
pub fn is_voice_type(channel_type: &str) -> bool {
    matches!(channel_type, "voice" | "stage")
}

pub async fn is_stage(state: &AppState, channel_id: &str) -> bool {
    db::channels::get_channel_row(&state.db, channel_id)
        .await
        .is_ok_and(|c| c.channel_type == "stage")
}

#[derive(Debug, Deserialize)]
pub struct StageVoiceUpdate {
    /// `false` brings the member up to speak, `true` sends them back to the
    /// audience.
    pub suppress: Option<bool>,
    /// Raise (`true`) or lower (`false`) one's own hand.
    pub request_to_speak: Option<bool>,
}

/// `PATCH /spaces/{space_id}/voice-states/{user_id}` (`@me` for oneself).
pub async fn update_stage_voice_state(
    State(state): State<AppState>,
    Path((space_id, user_id)): Path<(String, String)>,
    auth: AuthUser,
    Json(input): Json<StageVoiceUpdate>,
) -> Result<Json<serde_json::Value>, AppError> {
    let target = if user_id == "@me" {
        auth.user_id.clone()
    } else {
        user_id
    };
    let is_self = target == auth.user_id;
    let current = crate::voice::state::get_user_voice_state(&state, &target)
        .filter(|vs| vs.space_id.as_deref() == Some(space_id.as_str()))
        .ok_or_else(|| AppError::NotFound("user is not in voice in this space".into()))?;
    let channel_id = current.channel_id.clone().unwrap_or_default();
    if !is_stage(&state, &channel_id).await {
        return Err(AppError::BadRequest("channel_not_stage".into()));
    }
    let moderator = require_channel_permission(&state.db, &channel_id, &auth, "mute_members")
        .await
        .is_ok();
    if !is_self {
        if !moderator {
            return Err(AppError::Forbidden(
                "only stage moderators can change who speaks".into(),
            ));
        }
        if input.request_to_speak.is_some() {
            return Err(AppError::BadRequest("members raise their own hand".into()));
        }
    }
    if input.suppress == Some(false) && !moderator {
        return Err(AppError::Forbidden(
            "raise your hand and wait to be invited to speak".into(),
        ));
    }
    if input.request_to_speak == Some(true) {
        require_channel_permission(&state.db, &channel_id, &auth, "speak").await?;
    }

    let speakers = &state.security.stage_speakers;
    let mut request = current.request_to_speak_timestamp.clone();
    if let Some(raise) = input.request_to_speak {
        request = raise.then(db::polls::now_string);
    }
    match input.suppress {
        Some(false) => {
            speakers.insert(current.session_id.clone());
        }
        Some(true) => {
            speakers.remove(&current.session_id);
        }
        None => {}
    }
    // Speakers have nothing to ask for; going either way settles the hand.
    if input.suppress.is_some() || speakers.contains(&current.session_id) {
        request = None;
    }

    let request_changed = request != current.request_to_speak_timestamp;
    let updated = {
        let mut entry = state
            .voice_states
            .get_mut(&target)
            .filter(|e| e.session_id == current.session_id)
            .ok_or_else(|| AppError::NotFound("user is not in voice in this space".into()))?;
        entry.request_to_speak_timestamp = request;
        entry.clone()
    };
    crate::voice::moderation::sync_session(&state, &updated).await;
    let latest =
        crate::voice::state::get_user_voice_state(&state, &target).unwrap_or(updated.clone());
    // `sync_session` announces flag changes; a hand alone still needs saying.
    if request_changed && latest.suppress == updated.suppress {
        crate::routes::voice::broadcast_voice_state_update(
            &state,
            &channel_id,
            Some(&space_id),
            &latest,
        )
        .await;
    }
    Ok(Json(serde_json::json!({ "data": latest })))
}
