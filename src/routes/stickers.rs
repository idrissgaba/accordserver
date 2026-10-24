//! Stickers: images a space's members with `manage_emojis` upload, sent one
//! per message. A member may also send a sticker of another space they belong
//! to (with `use_external_stickers` or `use_external_emojis` in the channel).

use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;
use serde_json::Value;

use crate::db;
use crate::db::stickers::{NewSticker, Sticker};
use crate::error::AppError;
use crate::gateway::events::GatewayBroadcast;
use crate::middleware::auth::AuthUser;
use crate::middleware::permissions::{
    require_channel_permission, require_membership, require_not_timed_out, require_permission,
    slowmode_cooldown_ms,
};
use crate::models::message::CreateMessage;
use crate::state::AppState;
use crate::storage;

use super::messages::messages_to_json;

/// Most stickers per space.
pub const MAX_STICKERS: i64 = 60;

#[derive(Debug, Deserialize)]
pub struct CreateSticker {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Option<String>,
    /// The image as a `data:` URI (PNG, GIF or WebP).
    pub image: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct UpdateSticker {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct SendSticker {
    #[serde(default)]
    pub reply_to: Option<String>,
}

/// Checks and trims a sticker's texts: a 2–30 character name, a description of
/// up to 100 and a tag of up to 50 (empty ones dropped).
pub fn checked_texts(
    name: &str,
    description: Option<&str>,
    tags: Option<&str>,
) -> Result<(String, Option<String>, Option<String>), String> {
    let name = name.trim().to_string();
    if !(2..=30).contains(&name.chars().count()) {
        return Err("a sticker needs a name of 2-30 characters".into());
    }
    let clean = |value: Option<&str>, max: usize, what: &str| -> Result<Option<String>, String> {
        let value = value.map(str::trim).filter(|v| !v.is_empty());
        if value.is_some_and(|v| v.chars().count() > max) {
            return Err(format!("a sticker {what} has at most {max} characters"));
        }
        Ok(value.map(str::to_string))
    };
    Ok((
        name,
        clean(description, 100, "description")?,
        clean(tags, 50, "tag")?,
    ))
}

fn format_of(content_type: &str) -> Option<&'static str> {
    match content_type {
        "image/png" => Some("png"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        _ => None,
    }
}

async fn broadcast(state: &AppState, space_id: &str, event_type: &str, data: Value) {
    if let Some(ref tx) = *state.gateway_tx.read().await {
        let _ = tx.send(GatewayBroadcast {
            space_id: Some(space_id.to_string()),
            target_user_ids: None,
            event: serde_json::json!({ "op": 0, "type": event_type, "data": data }),
            intent: "emojis".to_string(),
            required_permission: None,
        });
    }
}

async fn space_sticker(
    state: &AppState,
    space_id: &str,
    sticker_id: &str,
) -> Result<Sticker, AppError> {
    let sticker = db::stickers::get(&state.db, sticker_id).await?;
    if sticker.space_id != space_id {
        return Err(AppError::NotFound("unknown sticker".into()));
    }
    Ok(sticker)
}

/// GET /spaces/{space_id}/stickers
pub async fn list_stickers(
    state: State<AppState>,
    Path(space_id): Path<String>,
    auth: AuthUser,
) -> Result<Json<Value>, AppError> {
    require_membership(&state.db, &space_id, &auth.user_id).await?;
    let stickers = db::stickers::list(&state.db, &space_id).await?;
    Ok(Json(serde_json::json!({ "data": stickers })))
}

/// POST /spaces/{space_id}/stickers
pub async fn create_sticker(
    state: State<AppState>,
    Path(space_id): Path<String>,
    auth: AuthUser,
    Json(input): Json<CreateSticker>,
) -> Result<Json<Value>, AppError> {
    require_permission(&state.db, &space_id, &auth, "manage_emojis").await?;
    let (name, description, tags) = checked_texts(
        &input.name,
        input.description.as_deref(),
        input.tags.as_deref(),
    )
    .map_err(AppError::BadRequest)?;
    if db::stickers::count(&state.db, &space_id).await? >= MAX_STICKERS {
        return Err(AppError::BadRequest(format!(
            "a space has at most {MAX_STICKERS} stickers"
        )));
    }
    let id = crate::snowflake::generate();
    let max = state.settings.load().max_emoji_size as usize;
    let (image, content_type, _, _) = storage::save_base64_image(
        &state,
        &auth.user_id,
        &space_id,
        &format!("sticker-{id}"),
        &input.image,
        max,
    )
    .await?;
    let Some(format) = format_of(&content_type) else {
        let _ = storage::delete_file(&state.storage_path, &image).await;
        return Err(AppError::BadRequest(
            "a sticker is a PNG, GIF or WebP image".into(),
        ));
    };
    let sticker = db::stickers::create(
        &state.db,
        &NewSticker {
            id: &id,
            space_id: &space_id,
            creator_id: &auth.user_id,
            name: &name,
            description: description.as_deref(),
            tags: tags.as_deref(),
            image: &image,
            format,
        },
    )
    .await?;
    let json = serde_json::to_value(&sticker).unwrap_or(Value::Null);
    broadcast(&state, &space_id, "sticker.create", json.clone()).await;
    Ok(Json(serde_json::json!({ "data": json })))
}

/// PATCH /spaces/{space_id}/stickers/{sticker_id}
pub async fn update_sticker(
    state: State<AppState>,
    Path((space_id, sticker_id)): Path<(String, String)>,
    auth: AuthUser,
    Json(input): Json<UpdateSticker>,
) -> Result<Json<Value>, AppError> {
    require_permission(&state.db, &space_id, &auth, "manage_emojis").await?;
    let sticker = space_sticker(&state, &space_id, &sticker_id).await?;
    let (name, description, tags) = checked_texts(
        input.name.as_deref().unwrap_or(&sticker.name),
        input
            .description
            .as_deref()
            .or(sticker.description.as_deref()),
        input.tags.as_deref().or(sticker.tags.as_deref()),
    )
    .map_err(AppError::BadRequest)?;
    db::stickers::update(
        &state.db,
        &sticker.id,
        &name,
        description.as_deref(),
        tags.as_deref(),
    )
    .await?;
    let updated = db::stickers::get(&state.db, &sticker.id).await?;
    let json = serde_json::to_value(&updated).unwrap_or(Value::Null);
    broadcast(&state, &space_id, "sticker.update", json.clone()).await;
    Ok(Json(serde_json::json!({ "data": json })))
}

/// DELETE /spaces/{space_id}/stickers/{sticker_id}
pub async fn delete_sticker(
    state: State<AppState>,
    Path((space_id, sticker_id)): Path<(String, String)>,
    auth: AuthUser,
) -> Result<Json<Value>, AppError> {
    require_permission(&state.db, &space_id, &auth, "manage_emojis").await?;
    let sticker = space_sticker(&state, &space_id, &sticker_id).await?;
    db::stickers::delete(&state.db, &sticker.id).await?;
    let _ = storage::delete_file(&state.storage_path, &sticker.image).await;
    broadcast(
        &state,
        &space_id,
        "sticker.delete",
        serde_json::json!({ "id": sticker.id, "space_id": space_id }),
    )
    .await;
    Ok(Json(serde_json::json!({ "data": null })))
}

/// POST /channels/{channel_id}/stickers/{sticker_id} — sends the sticker as
/// a message.
pub async fn send_sticker(
    state: State<AppState>,
    Path((channel_id, sticker_id)): Path<(String, String)>,
    auth: AuthUser,
    body: Option<Json<SendSticker>>,
) -> Result<Json<Value>, AppError> {
    let space_id =
        require_channel_permission(&state.db, &channel_id, &auth, "send_messages").await?;
    if space_id.is_empty() {
        return Err(AppError::BadRequest(
            "stickers are only available in space channels".into(),
        ));
    }
    require_not_timed_out(&state.db, &space_id, &auth).await?;
    let sticker = db::stickers::get(&state.db, &sticker_id).await?;
    if sticker.space_id != space_id {
        // Another space's sticker: the sender must belong there, and may use
        // outside stickers here.
        require_membership(&state.db, &sticker.space_id, &auth.user_id).await?;
        let external =
            require_channel_permission(&state.db, &channel_id, &auth, "use_external_stickers")
                .await
                .is_ok()
                || require_channel_permission(&state.db, &channel_id, &auth, "use_external_emojis")
                    .await
                    .is_ok();
        if !external {
            return Err(AppError::Forbidden(
                "missing permission: use_external_stickers".into(),
            ));
        }
    }
    let channel = db::channels::get_channel_row(&state.db, &channel_id).await?;
    let cooldown_ms = slowmode_cooldown_ms(&state.db, &channel, &auth).await?;
    let reply_to = body.and_then(|b| b.0.reply_to);
    let message = CreateMessage {
        content: String::new(),
        tts: None,
        embeds: None,
        reply_to,
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
    db::stickers::attach(&state.db, &row.id, &sticker.id).await?;
    let json = messages_to_json(&state.db, &[row], Some(&auth.user_id))
        .await?
        .into_iter()
        .next()
        .unwrap_or(Value::Null);
    if let Some(ref tx) = *state.gateway_tx.read().await {
        let _ = tx.send(GatewayBroadcast {
            space_id: Some(space_id),
            target_user_ids: None,
            event: serde_json::json!({ "op": 0, "type": "message.create", "data": json }),
            intent: "messages".to_string(),
            required_permission: None,
        });
    }
    Ok(Json(serde_json::json!({ "data": json })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sticker_texts_are_checked() {
        let (name, description, tags) = checked_texts(" Party ", Some("  "), Some(" 🎉 ")).unwrap();
        assert_eq!(name, "Party");
        assert_eq!(description, None);
        assert_eq!(tags.as_deref(), Some("🎉"));
        assert!(checked_texts("x", None, None).is_err());
        assert!(checked_texts(&"x".repeat(31), None, None).is_err());
        assert!(checked_texts("ok", Some(&"d".repeat(101)), None).is_err());
    }

    #[test]
    fn only_sticker_formats() {
        assert_eq!(format_of("image/png"), Some("png"));
        assert_eq!(format_of("image/gif"), Some("gif"));
        assert_eq!(format_of("image/jpeg"), None);
    }
}
