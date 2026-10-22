use axum::extract::{Path, State};
use axum::Json;

use crate::db;
use crate::error::AppError;
use crate::middleware::auth::AuthUser;
use crate::middleware::permissions::{
    require_channel_permission, require_not_timed_out, slowmode_cooldown_ms,
};
use crate::models::message::{CreateMessage, MessageRow};
use crate::models::poll::CreatePoll;
use crate::state::AppState;

use super::messages::{message_row_to_json_with_attachments, messages_to_json};

/// POST /channels/{channel_id}/polls — posts a new message carrying a poll.
///
/// Polls live in space channels: a direct message may be end-to-end
/// encrypted, and a poll's question and answers are not.
pub async fn create_poll(
    state: State<AppState>,
    Path(channel_id): Path<String>,
    auth: AuthUser,
    Json(input): Json<CreatePoll>,
) -> Result<Json<serde_json::Value>, AppError> {
    let space_id =
        require_channel_permission(&state.db, &channel_id, &auth, "send_messages").await?;
    if space_id.is_empty() {
        return Err(AppError::BadRequest(
            "polls are only available in space channels".into(),
        ));
    }
    require_not_timed_out(&state.db, &space_id, &auth).await?;
    input.validate().map_err(AppError::BadRequest)?;

    let channel = db::channels::get_channel_row(&state.db, &channel_id).await?;
    if crate::db::federation::space_origin(&state.db, &space_id)
        .await?
        .is_some()
    {
        return Err(AppError::BadRequest(
            "polls are not available in federated spaces".into(),
        ));
    }

    let cooldown_ms = slowmode_cooldown_ms(&state.db, &channel, &auth).await?;
    let message = CreateMessage {
        content: input.content.clone().unwrap_or_default(),
        tts: None,
        embeds: None,
        reply_to: None,
        thread_id: None,
        title: None,
    };
    let row = db::messages::create_message(
        &state.db,
        &channel_id,
        &auth.user_id,
        Some(&space_id),
        &message,
        cooldown_ms,
    )
    .await?;

    let expires_at = (chrono::Utc::now() + chrono::Duration::hours(input.duration()))
        .format("%Y-%m-%dT%H:%M:%S+00:00")
        .to_string();
    db::polls::create_poll(&state.db, &row.id, &input, &expires_at).await?;

    let ids = [row.id.clone()];
    let mut json = message_row_to_json_with_attachments(&row, &[], None);
    let broadcast_poll = db::polls::polls_for_messages(&state.db, &ids, None).await?;
    json["poll"] = broadcast_poll
        .get(&row.id)
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    if let Some(ref dispatcher) = *state.gateway_tx.read().await {
        let event = serde_json::json!({
            "op": 0,
            "type": "message.create",
            "data": json,
        });
        let _ = dispatcher.send(crate::gateway::events::GatewayBroadcast {
            space_id: channel.space_id.clone(),
            target_user_ids: None,
            event,
            intent: "messages".to_string(),
            required_permission: None,
        });
    }

    // The author's own copy says which answers they picked (none yet).
    let mine = db::polls::polls_for_messages(&state.db, &ids, Some(&auth.user_id)).await?;
    json["poll"] = mine
        .get(&row.id)
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    Ok(Json(serde_json::json!({ "data": json })))
}

/// PUT /channels/{channel_id}/polls/{message_id}/answers/{answer_id}/@me
pub async fn vote(
    state: State<AppState>,
    Path((channel_id, message_id, answer_id)): Path<(String, String, i64)>,
    auth: AuthUser,
) -> Result<Json<serde_json::Value>, AppError> {
    let (row, poll) = open_poll(&state, &channel_id, &message_id, &auth).await?;
    if !db::polls::answer_exists(&state.db, &message_id, answer_id).await? {
        return Err(AppError::NotFound("unknown poll answer".into()));
    }
    db::polls::vote(
        &state.db,
        &message_id,
        answer_id,
        &auth.user_id,
        poll.allow_multiselect,
    )
    .await?;
    poll_changed(&state, &row, &auth).await
}

/// DELETE /channels/{channel_id}/polls/{message_id}/answers/{answer_id}/@me
pub async fn unvote(
    state: State<AppState>,
    Path((channel_id, message_id, answer_id)): Path<(String, String, i64)>,
    auth: AuthUser,
) -> Result<Json<serde_json::Value>, AppError> {
    let (row, _) = open_poll(&state, &channel_id, &message_id, &auth).await?;
    db::polls::unvote(&state.db, &message_id, answer_id, &auth.user_id).await?;
    poll_changed(&state, &row, &auth).await
}

/// POST /channels/{channel_id}/polls/{message_id}/expire — ends the poll now.
/// Its author can, and so can anyone who may manage messages there.
pub async fn expire_poll(
    state: State<AppState>,
    Path((channel_id, message_id)): Path<(String, String)>,
    auth: AuthUser,
) -> Result<Json<serde_json::Value>, AppError> {
    let (row, _) = open_poll(&state, &channel_id, &message_id, &auth).await?;
    if row.author_id != auth.user_id {
        require_channel_permission(&state.db, &channel_id, &auth, "manage_messages").await?;
    }
    db::polls::expire(&state.db, &message_id, &db::polls::now_string()).await?;
    poll_changed(&state, &row, &auth).await
}

/// Loads [message_id]'s poll for a voter who can see [channel_id], refusing a
/// poll that has already closed.
async fn open_poll(
    state: &AppState,
    channel_id: &str,
    message_id: &str,
    auth: &AuthUser,
) -> Result<(MessageRow, db::polls::PollRow), AppError> {
    require_channel_permission(&state.db, channel_id, auth, "view_channel").await?;
    let row = db::messages::get_message_row(&state.db, message_id).await?;
    if row.channel_id != channel_id {
        return Err(AppError::NotFound("unknown_message".into()));
    }
    let poll = db::polls::get_poll(&state.db, message_id)
        .await?
        .ok_or_else(|| AppError::NotFound("this message has no poll".into()))?;
    if poll.is_closed(&db::polls::now_string()) {
        return Err(AppError::BadRequest("this poll has ended".into()));
    }
    Ok((row, poll))
}

/// Broadcasts the message with its new counts as a `message.update` and
/// returns the caller's view (with their own picks marked).
async fn poll_changed(
    state: &AppState,
    row: &MessageRow,
    auth: &AuthUser,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut broadcast = messages_to_json(&state.db, std::slice::from_ref(row), None).await?;
    if let Some(mut json) = broadcast.pop() {
        // Without a viewer, reactions would say nobody's own reaction is
        // theirs; leave them out so clients keep what they already have.
        if let Some(object) = json.as_object_mut() {
            object.remove("reactions");
        }
        if let Some(ref dispatcher) = *state.gateway_tx.read().await {
            let event = serde_json::json!({
                "op": 0,
                "type": "message.update",
                "data": json,
            });
            let _ = dispatcher.send(crate::gateway::events::GatewayBroadcast {
                space_id: row.space_id.clone(),
                target_user_ids: None,
                event,
                intent: "messages".to_string(),
                required_permission: None,
            });
        }
    }
    let mut mine =
        messages_to_json(&state.db, std::slice::from_ref(row), Some(&auth.user_id)).await?;
    Ok(Json(
        serde_json::json!({ "data": mine.pop().unwrap_or(serde_json::Value::Null) }),
    ))
}
