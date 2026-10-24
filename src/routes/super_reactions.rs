//! Super reactions: a showier reaction, with every perk free here. Stored and
//! announced apart from plain reactions (`super_reaction.add` / `.remove`), so
//! clients that don't know them see the same reactions as before.

use axum::extract::{Path, State};
use axum::Json;

use crate::db;
use crate::error::AppError;
use crate::gateway::events::GatewayBroadcast;
use crate::middleware::auth::AuthUser;
use crate::middleware::permissions::{require_channel_permission, require_not_timed_out};
use crate::state::AppState;

/// Checks the caller may react to `message_id` in `channel_id`; returns the
/// space id (`None` in DMs).
async fn reactable(
    state: &AppState,
    channel_id: &str,
    message_id: &str,
    auth: &AuthUser,
) -> Result<Option<String>, AppError> {
    let space_id = require_channel_permission(&state.db, channel_id, auth, "add_reactions").await?;
    let message = db::messages::get_message_row(&state.db, message_id).await?;
    if message.channel_id != channel_id {
        return Err(AppError::NotFound(
            "message not found in this channel".into(),
        ));
    }
    if space_id.is_empty() {
        return Ok(None);
    }
    require_not_timed_out(&state.db, &space_id, auth).await?;
    if db::federation::space_origin(&state.db, &space_id)
        .await?
        .is_some()
    {
        return Err(AppError::BadRequest(
            "super reactions stay on this server".into(),
        ));
    }
    Ok(Some(space_id))
}

async fn announce(
    state: &AppState,
    kind: &str,
    space_id: Option<String>,
    channel_id: &str,
    message_id: &str,
    user_id: &str,
    emoji: &str,
) {
    let targets = match space_id {
        Some(_) => None,
        None => Some(
            db::dm_participants::list_participant_ids(&state.db, channel_id)
                .await
                .unwrap_or_default(),
        ),
    };
    if let Some(ref tx) = *state.gateway_tx.read().await {
        let _ = tx.send(GatewayBroadcast {
            space_id,
            target_user_ids: targets,
            event: serde_json::json!({
                "op": 0,
                "type": kind,
                "data": {
                    "channel_id": channel_id,
                    "message_id": message_id,
                    "user_id": user_id,
                    "emoji": emoji,
                }
            }),
            intent: "message_reactions".to_string(),
            required_permission: None,
        });
    }
}

/// `PUT /channels/{channel_id}/messages/{message_id}/super-reactions/{emoji}`
pub async fn add_super_reaction(
    State(state): State<AppState>,
    Path((channel_id, message_id, emoji)): Path<(String, String, String)>,
    auth: AuthUser,
) -> Result<Json<serde_json::Value>, AppError> {
    if emoji.is_empty() || emoji.len() > 100 {
        return Err(AppError::BadRequest("invalid emoji".into()));
    }
    let space_id = reactable(&state, &channel_id, &message_id, &auth).await?;
    if db::super_reactions::add(
        &state.db,
        state.db_is_postgres,
        &message_id,
        &auth.user_id,
        &emoji,
    )
    .await?
    {
        announce(
            &state,
            "super_reaction.add",
            space_id,
            &channel_id,
            &message_id,
            &auth.user_id,
            &emoji,
        )
        .await;
    }
    Ok(Json(serde_json::json!({ "data": null })))
}

/// `DELETE /channels/{channel_id}/messages/{message_id}/super-reactions/{emoji}`
pub async fn remove_super_reaction(
    State(state): State<AppState>,
    Path((channel_id, message_id, emoji)): Path<(String, String, String)>,
    auth: AuthUser,
) -> Result<Json<serde_json::Value>, AppError> {
    let space_id = reactable(&state, &channel_id, &message_id, &auth).await?;
    if db::super_reactions::remove(&state.db, &message_id, &auth.user_id, &emoji).await? {
        announce(
            &state,
            "super_reaction.remove",
            space_id,
            &channel_id,
            &message_id,
            &auth.user_id,
            &emoji,
        )
        .await;
    }
    Ok(Json(serde_json::json!({ "data": null })))
}
