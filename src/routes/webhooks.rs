//! Incoming webhooks: members with `manage_webhooks` create a secret URL for a
//! text channel; anything holding the URL can post there. The execute payload
//! is the one chat services' webhook integrations already send (`content`,
//! `username`, `embeds`, `allowed_mentions`), so monitoring and media tools
//! can be pointed at it unchanged.

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;

use crate::db;
use crate::error::AppError;
use crate::gateway::events::GatewayBroadcast;
use crate::middleware::auth::AuthUser;
use crate::middleware::permissions::{require_channel_permission, require_permission};
use crate::models::message::{CreateMessage, MessageRow, UpdateMessage};
use crate::models::webhook::{
    lenient_embeds, validate_name, CreateWebhook, EditWebhookMessage, ExecuteWebhook,
    UpdateWebhook, Webhook, MAX_WEBHOOK_NAME,
};
use crate::state::AppState;
use crate::storage;

use super::messages::messages_to_json;

/// Most webhooks one channel may have.
const MAX_WEBHOOKS_PER_CHANNEL: usize = 15;

/// Longest webhook message, as for members' messages.
const MAX_CONTENT_CHARS: usize = 4000;

/// Most embeds in one webhook message.
const MAX_EMBEDS: usize = 10;

/// Channel types a webhook can post in.
fn postable(channel_type: &str) -> bool {
    matches!(channel_type, "text" | "announcement")
}

/// The webhook as JSON, with its secret only for those who manage it.
async fn webhook_json(state: &AppState, webhook: &Webhook, with_token: bool) -> serde_json::Value {
    let avatar = db::users::get_user(&state.db, &webhook.user_id)
        .await
        .ok()
        .and_then(|user| user.avatar);
    let mut json = serde_json::json!({
        "id": webhook.id,
        "space_id": webhook.space_id,
        "channel_id": webhook.channel_id,
        "user_id": webhook.user_id,
        "creator_id": webhook.creator_id,
        "name": webhook.name,
        "avatar": avatar,
        "created_at": webhook.created_at,
    });
    if with_token {
        json["token"] = serde_json::Value::String(webhook.token.clone());
    }
    json
}

/// Tells the space's managers a channel's webhooks changed.
async fn broadcast_webhooks_update(state: &AppState, space_id: &str, channel_id: &str) {
    if let Some(ref tx) = *state.gateway_tx.read().await {
        let _ = tx.send(GatewayBroadcast {
            space_id: Some(space_id.to_string()),
            target_user_ids: None,
            event: serde_json::json!({
                "op": 0,
                "type": "webhooks.update",
                "data": { "space_id": space_id, "channel_id": channel_id },
            }),
            intent: "spaces".to_string(),
            required_permission: Some("manage_webhooks"),
        });
    }
}

async fn audit(state: &AppState, webhook: &Webhook, actor: &str, action: &str) {
    if let Ok(entry) = db::audit_log::create_entry(
        &state.db,
        &webhook.space_id,
        actor,
        action,
        Some(&webhook.id),
        Some("webhook"),
        None,
        Some(
            &serde_json::json!({ "name": webhook.name, "channel_id": webhook.channel_id })
                .to_string(),
        ),
    )
    .await
    {
        super::audit_log::broadcast_entry(state, &entry).await;
    }
}

/// A space text channel a webhook may post in, for an actor who manages
/// webhooks there. Returns its space id.
async fn require_webhook_channel(
    state: &AppState,
    channel_id: &str,
    auth: &AuthUser,
) -> Result<String, AppError> {
    let space_id =
        require_channel_permission(&state.db, channel_id, auth, "manage_webhooks").await?;
    if space_id.is_empty() {
        return Err(AppError::BadRequest(
            "webhooks are only available in space channels".into(),
        ));
    }
    let channel = db::channels::get_channel_row(&state.db, channel_id).await?;
    if !postable(&channel.channel_type) {
        return Err(AppError::BadRequest(
            "webhooks can only post in text channels".into(),
        ));
    }
    if db::federation::space_origin(&state.db, &space_id)
        .await?
        .is_some()
    {
        return Err(AppError::BadRequest(
            "webhooks are not available in federated spaces".into(),
        ));
    }
    Ok(space_id)
}

/// Stores a `data:` URI avatar for the webhook's bot user, or clears it.
async fn apply_avatar(
    state: &AppState,
    webhook: &Webhook,
    actor: &str,
    avatar: &str,
) -> Result<(), AppError> {
    if avatar.is_empty() {
        storage::delete_avatar(&state.storage_path, "avatars", &webhook.user_id).await?;
        return db::webhooks::set_avatar(&state.db, webhook, None).await;
    }
    if !avatar.starts_with("data:") {
        return Err(AppError::BadRequest(
            "avatar must be a data: URI image".into(),
        ));
    }
    let max = state.settings.load().max_avatar_size as usize;
    let (url, _, _, _) = storage::save_avatar_image(
        state,
        actor,
        Some(&webhook.space_id),
        "avatars",
        &webhook.user_id,
        avatar,
        max,
    )
    .await?;
    db::webhooks::set_avatar(&state.db, webhook, Some(&url)).await
}

/// POST /channels/{channel_id}/webhooks
pub async fn create_webhook(
    state: State<AppState>,
    Path(channel_id): Path<String>,
    auth: AuthUser,
    Json(input): Json<CreateWebhook>,
) -> Result<Json<serde_json::Value>, AppError> {
    let space_id = require_webhook_channel(&state, &channel_id, &auth).await?;
    let name = validate_name(&input.name).map_err(AppError::BadRequest)?;
    if db::webhooks::list_for_channel(&state.db, &channel_id)
        .await?
        .len()
        >= MAX_WEBHOOKS_PER_CHANNEL
    {
        return Err(AppError::BadRequest(format!(
            "a channel can have at most {MAX_WEBHOOKS_PER_CHANNEL} webhooks"
        )));
    }
    let webhook =
        db::webhooks::create(&state.db, &space_id, &channel_id, &auth.user_id, &name).await?;
    if let Some(avatar) = input.avatar.as_deref().filter(|a| !a.is_empty()) {
        if let Err(err) = apply_avatar(&state, &webhook, &auth.user_id, avatar).await {
            db::webhooks::delete(&state.db, &webhook.id).await?;
            return Err(err);
        }
    }
    audit(&state, &webhook, &auth.user_id, "webhook_create").await;
    broadcast_webhooks_update(&state, &space_id, &channel_id).await;
    Ok(Json(
        serde_json::json!({ "data": webhook_json(&state, &webhook, true).await }),
    ))
}

/// GET /channels/{channel_id}/webhooks
pub async fn list_channel_webhooks(
    state: State<AppState>,
    Path(channel_id): Path<String>,
    auth: AuthUser,
) -> Result<Json<serde_json::Value>, AppError> {
    require_webhook_channel(&state, &channel_id, &auth).await?;
    let mut data = Vec::new();
    for webhook in db::webhooks::list_for_channel(&state.db, &channel_id).await? {
        data.push(webhook_json(&state, &webhook, true).await);
    }
    Ok(Json(serde_json::json!({ "data": data })))
}

/// GET /spaces/{space_id}/webhooks
pub async fn list_space_webhooks(
    state: State<AppState>,
    Path(space_id): Path<String>,
    auth: AuthUser,
) -> Result<Json<serde_json::Value>, AppError> {
    require_permission(&state.db, &space_id, &auth, "manage_webhooks").await?;
    let mut data = Vec::new();
    for webhook in db::webhooks::list_for_space(&state.db, &space_id).await? {
        data.push(webhook_json(&state, &webhook, true).await);
    }
    Ok(Json(serde_json::json!({ "data": data })))
}

/// The webhook `webhook_id`, for an actor who manages webhooks in its channel.
async fn managed_webhook(
    state: &AppState,
    webhook_id: &str,
    auth: &AuthUser,
) -> Result<Webhook, AppError> {
    let webhook = db::webhooks::get(&state.db, webhook_id).await?;
    require_channel_permission(&state.db, &webhook.channel_id, auth, "manage_webhooks").await?;
    Ok(webhook)
}

/// GET /webhooks/{webhook_id}
pub async fn get_webhook(
    state: State<AppState>,
    Path(webhook_id): Path<String>,
    auth: AuthUser,
) -> Result<Json<serde_json::Value>, AppError> {
    let webhook = managed_webhook(&state, &webhook_id, &auth).await?;
    Ok(Json(
        serde_json::json!({ "data": webhook_json(&state, &webhook, true).await }),
    ))
}

/// PATCH /webhooks/{webhook_id}
pub async fn update_webhook(
    state: State<AppState>,
    Path(webhook_id): Path<String>,
    auth: AuthUser,
    Json(input): Json<UpdateWebhook>,
) -> Result<Json<serde_json::Value>, AppError> {
    let webhook = managed_webhook(&state, &webhook_id, &auth).await?;
    let old_channel = webhook.channel_id.clone();
    if let Some(ref channel_id) = input.channel_id {
        if channel_id != &webhook.channel_id {
            let space_id = require_webhook_channel(&state, channel_id, &auth).await?;
            if space_id != webhook.space_id {
                return Err(AppError::BadRequest(
                    "a webhook can only move within its space".into(),
                ));
            }
            db::webhooks::move_to_channel(&state.db, &webhook.id, channel_id).await?;
        }
    }
    if let Some(ref name) = input.name {
        let name = validate_name(name).map_err(AppError::BadRequest)?;
        db::webhooks::rename(&state.db, &webhook, &name).await?;
    }
    if let Some(ref avatar) = input.avatar {
        apply_avatar(&state, &webhook, &auth.user_id, avatar).await?;
    }
    let updated = db::webhooks::get(&state.db, &webhook.id).await?;
    audit(&state, &updated, &auth.user_id, "webhook_update").await;
    broadcast_webhooks_update(&state, &updated.space_id, &old_channel).await;
    if updated.channel_id != old_channel {
        broadcast_webhooks_update(&state, &updated.space_id, &updated.channel_id).await;
    }
    Ok(Json(
        serde_json::json!({ "data": webhook_json(&state, &updated, true).await }),
    ))
}

/// DELETE /webhooks/{webhook_id}
pub async fn delete_webhook(
    state: State<AppState>,
    Path(webhook_id): Path<String>,
    auth: AuthUser,
) -> Result<Json<serde_json::Value>, AppError> {
    let webhook = managed_webhook(&state, &webhook_id, &auth).await?;
    db::webhooks::delete(&state.db, &webhook.id).await?;
    audit(&state, &webhook, &auth.user_id, "webhook_delete").await;
    broadcast_webhooks_update(&state, &webhook.space_id, &webhook.channel_id).await;
    Ok(Json(serde_json::json!({ "data": null })))
}

/// The webhook behind a URL, or 404 for a wrong id or secret alike.
async fn webhook_by_token(
    state: &AppState,
    webhook_id: &str,
    token: &str,
) -> Result<Webhook, AppError> {
    let unknown = || AppError::NotFound("unknown webhook".into());
    let webhook = db::webhooks::get(&state.db, webhook_id)
        .await
        .map_err(|_| unknown())?;
    if !db::webhooks::token_matches(&webhook.token, token) {
        return Err(unknown());
    }
    Ok(webhook)
}

/// GET /webhooks/{webhook_id}/{token} — lets an integration check its URL.
pub async fn get_webhook_with_token(
    state: State<AppState>,
    Path((webhook_id, token)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, AppError> {
    let webhook = webhook_by_token(&state, &webhook_id, &token).await?;
    Ok(Json(webhook_json(&state, &webhook, false).await))
}

#[derive(Debug, Default, Deserialize)]
pub struct ExecuteQuery {
    /// Answer with the created message instead of an empty 204.
    #[serde(default)]
    pub wait: Option<bool>,
}

fn parse_body<T: serde::de::DeserializeOwned>(body: &Bytes) -> Result<T, AppError> {
    serde_json::from_slice(body)
        .map_err(|e| AppError::BadRequest(format!("invalid webhook payload: {e}")))
}

fn check_content(content: &str) -> Result<(), AppError> {
    if content.chars().count() > MAX_CONTENT_CHARS {
        return Err(AppError::BadRequest(format!(
            "content must be at most {MAX_CONTENT_CHARS} characters"
        )));
    }
    Ok(())
}

async fn message_json(state: &AppState, row: MessageRow) -> Result<serde_json::Value, AppError> {
    Ok(messages_to_json(&state.db, &[row], None)
        .await?
        .into_iter()
        .next()
        .unwrap_or(serde_json::Value::Null))
}

async fn broadcast_message(
    state: &AppState,
    event_type: &str,
    space_id: &str,
    data: serde_json::Value,
) {
    if let Some(ref tx) = *state.gateway_tx.read().await {
        let _ = tx.send(GatewayBroadcast {
            space_id: Some(space_id.to_string()),
            target_user_ids: None,
            event: serde_json::json!({ "op": 0, "type": event_type, "data": data }),
            intent: "messages".to_string(),
            required_permission: None,
        });
    }
}

/// POST /webhooks/{webhook_id}/{token} — posts a message as the webhook.
pub async fn execute_webhook(
    state: State<AppState>,
    Path((webhook_id, token)): Path<(String, String)>,
    Query(query): Query<ExecuteQuery>,
    body: Bytes,
) -> Result<Response, AppError> {
    let webhook = webhook_by_token(&state, &webhook_id, &token).await?;
    let input: ExecuteWebhook = parse_body(&body)?;

    let content = input.content.clone().unwrap_or_default();
    check_content(&content)?;
    let embeds = lenient_embeds(input.embeds.as_deref().unwrap_or_default());
    if embeds.len() > MAX_EMBEDS {
        return Err(AppError::BadRequest(format!(
            "at most {MAX_EMBEDS} embeds per message"
        )));
    }
    if content.trim().is_empty() && embeds.is_empty() {
        return Err(AppError::BadRequest("cannot send an empty message".into()));
    }
    let username = match input.username.as_deref().map(str::trim) {
        Some(name) if !name.is_empty() => {
            if name.chars().count() > MAX_WEBHOOK_NAME {
                return Err(AppError::BadRequest(format!(
                    "username must be at most {MAX_WEBHOOK_NAME} characters"
                )));
            }
            Some(name.to_string())
        }
        _ => None,
    };

    let channel = db::channels::get_channel_row(&state.db, &webhook.channel_id).await?;
    if channel.space_id.as_deref() != Some(webhook.space_id.as_str()) {
        return Err(AppError::NotFound("unknown webhook".into()));
    }
    let message = CreateMessage {
        content,
        tts: input.tts,
        embeds: Some(embeds),
        reply_to: None,
        thread_id: None,
        title: None,
    };
    let row = db::messages::create_message(
        &state.db,
        &webhook.channel_id,
        &webhook.user_id,
        Some(&webhook.space_id),
        &message,
        0,
    )
    .await?;
    db::webhooks::record_message(
        &state.db,
        &row.id,
        &webhook.id,
        username.as_deref(),
        input.may_mention_everyone(),
    )
    .await?;
    let row = db::messages::get_message_row(&state.db, &row.id).await?;
    super::messages::apply_mention_counts(&state, &row).await;

    let json = message_json(&state, row).await?;
    broadcast_message(&state, "message.create", &webhook.space_id, json.clone()).await;

    if query.wait.unwrap_or(false) {
        Ok(Json(json).into_response())
    } else {
        Ok(StatusCode::NO_CONTENT.into_response())
    }
}

/// A message this webhook posted, from its URL.
async fn own_message(
    state: &AppState,
    webhook: &Webhook,
    message_id: &str,
) -> Result<MessageRow, AppError> {
    let row = db::messages::get_message_row(&state.db, message_id).await?;
    if row.webhook_id.as_deref() != Some(webhook.id.as_str()) {
        return Err(AppError::NotFound("unknown_message".into()));
    }
    Ok(row)
}

/// PATCH /webhooks/{webhook_id}/{token}/messages/{message_id}
pub async fn edit_webhook_message(
    state: State<AppState>,
    Path((webhook_id, token, message_id)): Path<(String, String, String)>,
    body: Bytes,
) -> Result<Json<serde_json::Value>, AppError> {
    let webhook = webhook_by_token(&state, &webhook_id, &token).await?;
    let row = own_message(&state, &webhook, &message_id).await?;
    let input: EditWebhookMessage = parse_body(&body)?;
    if let Some(ref content) = input.content {
        check_content(content)?;
    }
    let embeds = input.embeds.as_deref().map(lenient_embeds);
    if embeds.as_ref().is_some_and(|e| e.len() > MAX_EMBEDS) {
        return Err(AppError::BadRequest(format!(
            "at most {MAX_EMBEDS} embeds per message"
        )));
    }
    let update = UpdateMessage {
        content: input.content,
        embeds,
        title: None,
    };
    let updated =
        db::messages::update_message(&state.db, &row.id, &update, state.db_is_postgres).await?;
    let json = message_json(&state, updated).await?;
    broadcast_message(&state, "message.update", &webhook.space_id, json.clone()).await;
    Ok(Json(json))
}

/// DELETE /webhooks/{webhook_id}/{token}/messages/{message_id}
pub async fn delete_webhook_message(
    state: State<AppState>,
    Path((webhook_id, token, message_id)): Path<(String, String, String)>,
) -> Result<Response, AppError> {
    let webhook = webhook_by_token(&state, &webhook_id, &token).await?;
    let row = own_message(&state, &webhook, &message_id).await?;
    db::messages::delete_message(&state.db, &row.id).await?;
    broadcast_message(
        &state,
        "message.delete",
        &webhook.space_id,
        serde_json::json!({
            "id": row.id,
            "channel_id": row.channel_id,
            "space_id": webhook.space_id,
        }),
    )
    .await;
    Ok(StatusCode::NO_CONTENT.into_response())
}
