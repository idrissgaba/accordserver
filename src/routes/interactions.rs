//! Slash commands and message components.
//!
//! Applications (bots) register commands; members discover them per space and
//! invoke them (`POST /interactions`), or click a button / pick from a select
//! menu on a bot's message. The server checks the invocation and sends the
//! bot an `interaction.create` gateway event carrying a token; with it, the
//! bot answers (`/interactions/{id}/{token}/callback`) with a message, a
//! "thinking…" placeholder it fills in later, an update of the clicked
//! message, or an answer only the member sees (ephemeral).

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::Value;

use crate::db;
use crate::db::app_commands::{AppInfo, CommandRow};
use crate::db::interactions::{InteractionRow, FLAG_EPHEMERAL, FLAG_LOADING};
use crate::error::AppError;
use crate::gateway::events::GatewayBroadcast;
use crate::middleware::auth::AuthUser;
use crate::middleware::permissions::{
    require_channel_permission, require_membership, require_not_timed_out,
};
use crate::models::app_command::{
    find_component, resolve_options, validate_components, CommandDefinition, OptionValue,
    MAX_COMMANDS_PER_APPLICATION,
};
use crate::models::embed::Embed;
use crate::models::message::{CreateMessage, MessageRow, UpdateMessage};
use crate::state::AppState;

use super::messages::messages_to_json;

const MAX_CONTENT_CHARS: usize = 4000;
const MAX_EMBEDS: usize = 10;

// ── Gateway helpers ─────────────────────────────────────────────────────────

async fn send(state: &AppState, broadcast: GatewayBroadcast) {
    if let Some(ref tx) = *state.gateway_tx.read().await {
        let _ = tx.send(broadcast);
    }
}

async fn to_space(state: &AppState, space_id: &str, event_type: &str, data: Value) {
    send(
        state,
        GatewayBroadcast {
            space_id: Some(space_id.to_string()),
            target_user_ids: None,
            event: serde_json::json!({ "op": 0, "type": event_type, "data": data }),
            intent: "messages".to_string(),
            required_permission: None,
        },
    )
    .await;
}

async fn to_user(state: &AppState, user_id: &str, event_type: &str, data: Value) {
    send(
        state,
        GatewayBroadcast {
            space_id: None,
            target_user_ids: Some(vec![user_id.to_string()]),
            event: serde_json::json!({ "op": 0, "type": event_type, "data": data }),
            intent: "messages".to_string(),
            required_permission: None,
        },
    )
    .await;
}

/// Tells members of every space the application's bot is in that its
/// commands changed, so command pickers refresh.
async fn announce_commands(state: &AppState, app: &AppInfo) {
    let Some(ref bot) = app.bot_user_id else {
        return;
    };
    for space_id in db::app_commands::spaces_of_user(&state.db, bot)
        .await
        .unwrap_or_default()
    {
        to_space(
            state,
            &space_id,
            "application_commands.update",
            serde_json::json!({ "space_id": space_id, "application_id": app.id }),
        )
        .await;
    }
}

// ── Registration (the application's bot or owner) ───────────────────────────

fn parse_definition(value: Value) -> Result<CommandDefinition, AppError> {
    serde_json::from_value::<CommandDefinition>(value)
        .map_err(|e| AppError::BadRequest(format!("invalid command: {e}")))?
        .validate()
        .map_err(AppError::BadRequest)
}

fn commands_json(rows: &[CommandRow]) -> Value {
    Value::Array(rows.iter().map(CommandRow::to_json).collect())
}

/// GET /applications/{app_id}/commands
pub async fn list_global_commands(
    state: State<AppState>,
    Path(app_id): Path<String>,
    auth: AuthUser,
) -> Result<Json<Value>, AppError> {
    db::app_commands::application_for_caller(&state.db, &app_id, &auth.user_id).await?;
    let rows = db::app_commands::list_commands(&state.db, &app_id, None).await?;
    Ok(Json(serde_json::json!({ "data": commands_json(&rows) })))
}

/// POST /applications/{app_id}/commands — creates the command, or replaces
/// the one with the same name.
pub async fn create_global_command(
    state: State<AppState>,
    Path(app_id): Path<String>,
    auth: AuthUser,
    Json(body): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let app = db::app_commands::application_for_caller(&state.db, &app_id, &auth.user_id).await?;
    let def = parse_definition(body)?;
    let existing = db::app_commands::list_commands(&state.db, &app_id, None).await?;
    if existing.len() >= MAX_COMMANDS_PER_APPLICATION
        && !existing.iter().any(|c| c.name == def.name)
    {
        return Err(AppError::BadRequest(format!(
            "an application has at most {MAX_COMMANDS_PER_APPLICATION} commands"
        )));
    }
    let row = db::app_commands::upsert_command(&state.db, &app_id, None, &def).await?;
    announce_commands(&state, &app).await;
    Ok(Json(serde_json::json!({ "data": row.to_json() })))
}

/// PUT /applications/{app_id}/commands — makes the application's commands
/// exactly the list given (what a bot does at startup).
pub async fn replace_global_commands(
    state: State<AppState>,
    Path(app_id): Path<String>,
    auth: AuthUser,
    Json(body): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let app = db::app_commands::application_for_caller(&state.db, &app_id, &auth.user_id).await?;
    let list = body
        .as_array()
        .ok_or_else(|| AppError::BadRequest("expected a list of commands".into()))?;
    if list.len() > MAX_COMMANDS_PER_APPLICATION {
        return Err(AppError::BadRequest(format!(
            "an application has at most {MAX_COMMANDS_PER_APPLICATION} commands"
        )));
    }
    let mut defs: Vec<CommandDefinition> = Vec::new();
    for value in list {
        let def = parse_definition(value.clone())?;
        if defs.iter().any(|d| d.name == def.name) {
            return Err(AppError::BadRequest(format!(
                "command '{}' is listed twice",
                def.name
            )));
        }
        defs.push(def);
    }
    let rows = db::app_commands::replace_commands(&state.db, &app_id, None, &defs).await?;
    announce_commands(&state, &app).await;
    Ok(Json(serde_json::json!({ "data": commands_json(&rows) })))
}

/// DELETE /applications/{app_id}/commands/{command_id}
pub async fn delete_global_command(
    state: State<AppState>,
    Path((app_id, command_id)): Path<(String, String)>,
    auth: AuthUser,
) -> Result<Json<Value>, AppError> {
    let app = db::app_commands::application_for_caller(&state.db, &app_id, &auth.user_id).await?;
    db::app_commands::delete_command(&state.db, &app_id, &command_id).await?;
    announce_commands(&state, &app).await;
    Ok(Json(serde_json::json!({ "data": null })))
}

/// GET /spaces/{space_id}/commands — what the command picker offers there.
pub async fn list_space_commands(
    state: State<AppState>,
    Path(space_id): Path<String>,
    auth: AuthUser,
) -> Result<Json<Value>, AppError> {
    require_membership(&state.db, &space_id, &auth.user_id).await?;
    let mut data = Vec::new();
    for (command, app) in db::app_commands::commands_for_space(&state.db, &space_id).await? {
        let mut json = command.to_json();
        json["application"] = serde_json::json!({
            "id": app.id,
            "name": app.name,
            "bot_user_id": app.bot_user_id,
        });
        data.push(json);
    }
    Ok(Json(serde_json::json!({ "data": data })))
}

// ── Invocation (members) ────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct InteractionRequest {
    /// `application_command` or `message_component`.
    #[serde(rename = "type")]
    pub kind: String,
    pub channel_id: String,
    /// The command to run (`application_command`).
    #[serde(default)]
    pub command_id: Option<String>,
    /// The message whose component was used (`message_component`).
    #[serde(default)]
    pub message_id: Option<String>,
    /// `{options: [{name, value}]}` for a command; `{custom_id, values?}` for
    /// a component.
    #[serde(default)]
    pub data: Value,
}

async fn user_summary(state: &AppState, user_id: &str) -> Value {
    match db::users::get_user(&state.db, user_id).await {
        Ok(user) => serde_json::json!({
            "id": user.id,
            "username": user.username,
            "display_name": user.display_name,
            "avatar": user.avatar,
        }),
        Err(_) => serde_json::json!({ "id": user_id }),
    }
}

fn bot_of(app: &AppInfo) -> Result<String, AppError> {
    app.bot_user_id
        .clone()
        .ok_or_else(|| AppError::BadRequest("that application has no bot".into()))
}

/// Checks a resolved user / channel / role id belongs to the space.
async fn check_reference(
    state: &AppState,
    space_id: &str,
    option_type: &str,
    id: &str,
) -> Result<(), AppError> {
    let ok = match option_type {
        "user" => db::members::get_member_row(&state.db, space_id, id)
            .await
            .is_ok(),
        "channel" => db::channels::get_channel_row(&state.db, id)
            .await
            .is_ok_and(|c| c.space_id.as_deref() == Some(space_id)),
        "role" => db::roles::get_role_row(&state.db, id)
            .await
            .is_ok_and(|r| r.space_id == space_id),
        _ => true,
    };
    if ok {
        Ok(())
    } else {
        Err(AppError::BadRequest(format!(
            "that {option_type} is not in this space"
        )))
    }
}

/// The bot message a component interaction targets, as JSON with its
/// components, and its author: a stored message, or an ephemeral answer only
/// this member got.
async fn source_message(
    state: &AppState,
    channel_id: &str,
    message_id: &str,
    user_id: &str,
) -> Result<(Value, String), AppError> {
    if let Ok(row) = db::messages::get_message_row(&state.db, message_id).await {
        if row.channel_id != channel_id {
            return Err(AppError::NotFound("unknown_message".into()));
        }
        let author = row.author_id.clone();
        let json = messages_to_json(&state.db, &[row], Some(user_id))
            .await?
            .into_iter()
            .next()
            .unwrap_or(Value::Null);
        return Ok((json, author));
    }
    let earlier = db::interactions::get(&state.db, message_id)
        .await
        .map_err(|_| AppError::NotFound("unknown_message".into()))?;
    let stored = earlier
        .ephemeral_response
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .filter(|_| earlier.user_id == user_id && earlier.channel_id == channel_id)
        .ok_or_else(|| AppError::NotFound("unknown_message".into()))?;
    let author = stored["author_id"].as_str().unwrap_or_default().to_string();
    Ok((stored, author))
}

/// What a checked invocation resolves to.
struct Invocation {
    app: AppInfo,
    kind: &'static str,
    name: Option<String>,
    source_message_id: Option<String>,
    data: Value,
    message: Option<Value>,
}

async fn command_invocation(
    state: &AppState,
    space_id: &str,
    input: &InteractionRequest,
) -> Result<Invocation, AppError> {
    let command_id = input
        .command_id
        .as_deref()
        .ok_or_else(|| AppError::BadRequest("command_id is required".into()))?;
    let command = db::app_commands::get_command(&state.db, command_id).await?;
    if command.space_id.as_deref().is_some_and(|s| s != space_id) {
        return Err(AppError::NotFound("unknown command".into()));
    }
    let app = db::app_commands::get_application(&state.db, &command.application_id).await?;
    let bot = bot_of(&app)?;
    if db::members::get_member_row(&state.db, space_id, &bot)
        .await
        .is_err()
    {
        return Err(AppError::BadRequest(
            "that application is not in this space".into(),
        ));
    }
    let given: Vec<OptionValue> = match input.data.get("options") {
        None | Some(Value::Null) => Vec::new(),
        Some(raw) => serde_json::from_value(raw.clone())
            .map_err(|e| AppError::BadRequest(format!("invalid options: {e}")))?,
    };
    let resolved = resolve_options(&command.definition(), &given).map_err(AppError::BadRequest)?;
    for option in &resolved {
        if let Some(id) = option.value.as_str() {
            check_reference(state, space_id, &option.option_type, id).await?;
        }
    }
    Ok(Invocation {
        app,
        kind: "application_command",
        name: Some(command.name.clone()),
        source_message_id: None,
        data: serde_json::json!({
            "id": command.id,
            "name": command.name,
            "options": resolved,
        }),
        message: None,
    })
}

async fn component_invocation(
    state: &AppState,
    input: &InteractionRequest,
    user_id: &str,
) -> Result<Invocation, AppError> {
    let message_id = input
        .message_id
        .clone()
        .ok_or_else(|| AppError::BadRequest("message_id is required".into()))?;
    let custom_id = input
        .data
        .get("custom_id")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::BadRequest("custom_id is required".into()))?
        .to_string();
    let (message, author) = source_message(state, &input.channel_id, &message_id, user_id).await?;
    let app = db::app_commands::application_by_bot_user(&state.db, &author)
        .await?
        .ok_or_else(|| AppError::BadRequest("that message has no components".into()))?;
    let component = find_component(&message["components"], &custom_id)
        .ok_or_else(|| AppError::BadRequest("unknown component".into()))?
        .clone();
    if component["disabled"].as_bool() == Some(true) {
        return Err(AppError::BadRequest("that component is disabled".into()));
    }
    let component_type = component["type"].as_str().unwrap_or("button").to_string();
    let mut data = serde_json::json!({
        "custom_id": custom_id,
        "component_type": component_type,
    });
    if component_type == "select" {
        let values: Vec<String> = input
            .data
            .get("values")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        let allowed: Vec<&str> = component["options"]
            .as_array()
            .map(|o| o.iter().filter_map(|x| x["value"].as_str()).collect())
            .unwrap_or_default();
        let min = component["min_values"].as_u64().unwrap_or(1) as usize;
        let max = component["max_values"].as_u64().unwrap_or(1) as usize;
        if values.len() < min
            || values.len() > max
            || values.iter().any(|v| !allowed.contains(&v.as_str()))
        {
            return Err(AppError::BadRequest("invalid selection".into()));
        }
        data["values"] = serde_json::json!(values);
    }
    Ok(Invocation {
        app,
        kind: "message_component",
        name: None,
        source_message_id: Some(message_id),
        data,
        message: Some(message),
    })
}

/// POST /interactions — a member uses a slash command or a component.
pub async fn create_interaction(
    state: State<AppState>,
    auth: AuthUser,
    Json(input): Json<InteractionRequest>,
) -> Result<Json<Value>, AppError> {
    let space_id =
        require_channel_permission(&state.db, &input.channel_id, &auth, "send_messages").await?;
    if space_id.is_empty() {
        return Err(AppError::BadRequest(
            "commands are only available in space channels".into(),
        ));
    }
    require_not_timed_out(&state.db, &space_id, &auth).await?;

    let invocation = match input.kind.as_str() {
        "application_command" => command_invocation(&state, &space_id, &input).await?,
        "message_component" => component_invocation(&state, &input, &auth.user_id).await?,
        _ => {
            return Err(AppError::BadRequest(
                "type must be application_command or message_component".into(),
            ))
        }
    };

    let bot = bot_of(&invocation.app)?;
    let row = db::interactions::create(
        &state.db,
        &db::interactions::NewInteraction {
            application_id: &invocation.app.id,
            interaction_type: invocation.kind,
            user_id: &auth.user_id,
            space_id: &space_id,
            channel_id: &input.channel_id,
            name: invocation.name.as_deref(),
            source_message_id: invocation.source_message_id.as_deref(),
        },
    )
    .await?;
    let mut payload = serde_json::json!({
        "id": row.id,
        "application_id": invocation.app.id,
        "type": invocation.kind,
        "token": row.token,
        "space_id": space_id,
        "channel_id": input.channel_id,
        "user": user_summary(&state, &auth.user_id).await,
        "data": invocation.data,
    });
    if let Some(message) = invocation.message {
        payload["message"] = message;
    }
    send(
        &state,
        GatewayBroadcast {
            space_id: None,
            target_user_ids: Some(vec![bot]),
            event: serde_json::json!({ "op": 0, "type": "interaction.create", "data": payload }),
            intent: "messages".to_string(),
            required_permission: None,
        },
    )
    .await;
    Ok(Json(serde_json::json!({ "data": { "id": row.id } })))
}

// ── Answers (the application, by interaction token) ─────────────────────────

/// A message an application answers with (initial answer, edit or follow-up).
#[derive(Debug, Default, Deserialize)]
pub struct ResponseData {
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub embeds: Option<Vec<Embed>>,
    #[serde(default)]
    pub components: Option<Value>,
    #[serde(default)]
    pub flags: Option<i64>,
    #[serde(default)]
    pub tts: Option<bool>,
}

impl ResponseData {
    fn ephemeral(&self) -> bool {
        self.flags.unwrap_or(0) & FLAG_EPHEMERAL != 0
    }

    /// Checks sizes and normalises the components.
    fn checked(mut self, require_something: bool) -> Result<Self, AppError> {
        if let Some(ref content) = self.content {
            if content.chars().count() > MAX_CONTENT_CHARS {
                return Err(AppError::BadRequest(format!(
                    "content must be at most {MAX_CONTENT_CHARS} characters"
                )));
            }
        }
        if self.embeds.as_ref().is_some_and(|e| e.len() > MAX_EMBEDS) {
            return Err(AppError::BadRequest(format!(
                "at most {MAX_EMBEDS} embeds per message"
            )));
        }
        if let Some(ref raw) = self.components {
            self.components = if raw.is_null() {
                Some(Value::Array(Vec::new()))
            } else {
                Some(validate_components(raw).map_err(AppError::BadRequest)?)
            };
        }
        if require_something
            && self
                .content
                .as_deref()
                .unwrap_or_default()
                .trim()
                .is_empty()
            && self.embeds.as_ref().is_none_or(|e| e.is_empty())
            && self
                .components
                .as_ref()
                .is_none_or(|c| c.as_array().is_none_or(|a| a.is_empty()))
        {
            return Err(AppError::BadRequest("cannot send an empty message".into()));
        }
        Ok(self)
    }
}

#[derive(Debug, Deserialize)]
pub struct CallbackBody {
    #[serde(rename = "type")]
    pub kind: Value,
    #[serde(default)]
    pub data: Option<ResponseData>,
}

fn callback_kind(raw: &Value) -> Option<&'static str> {
    match raw {
        Value::String(word) => match word.as_str() {
            "pong" => Some("pong"),
            "message" | "channel_message_with_source" => Some("message"),
            "deferred_message" | "deferred_channel_message_with_source" => Some("deferred_message"),
            "deferred_update" | "deferred_update_message" => Some("deferred_update"),
            "update_message" => Some("update_message"),
            _ => None,
        },
        Value::Number(code) => match code.as_i64()? {
            1 => Some("pong"),
            4 => Some("message"),
            5 => Some("deferred_message"),
            6 => Some("deferred_update"),
            7 => Some("update_message"),
            _ => None,
        },
        _ => None,
    }
}

fn parse_json<T: serde::de::DeserializeOwned>(body: &Bytes) -> Result<T, AppError> {
    serde_json::from_slice(body).map_err(|e| AppError::BadRequest(format!("invalid body: {e}")))
}

/// Compares a presented token with the stored one in constant time.
fn token_eq(stored: &str, presented: &str) -> bool {
    stored.len() == presented.len()
        && stored
            .bytes()
            .zip(presented.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

/// The interaction behind an answer URL, while it can still be answered.
async fn interaction_by_token(
    state: &AppState,
    id: &str,
    token: &str,
) -> Result<InteractionRow, AppError> {
    let unknown = || AppError::NotFound("unknown interaction".into());
    let row = db::interactions::get(&state.db, id)
        .await
        .map_err(|_| unknown())?;
    if !token_eq(&row.token, token) || !db::interactions::is_live(&row, chrono::Utc::now()) {
        return Err(unknown());
    }
    Ok(row)
}

async fn message_json(state: &AppState, row: MessageRow) -> Result<Value, AppError> {
    Ok(messages_to_json(&state.db, &[row], None)
        .await?
        .into_iter()
        .next()
        .unwrap_or(Value::Null))
}

/// Posts `data` as the bot in the interaction's channel, optionally still
/// loading and marked as answering it (`interaction`).
async fn post_answer(
    state: &AppState,
    interaction: &InteractionRow,
    bot: &str,
    data: &ResponseData,
    loading: bool,
    with_header: bool,
) -> Result<Value, AppError> {
    let message = CreateMessage {
        content: data.content.clone().unwrap_or_default(),
        tts: data.tts,
        embeds: data.embeds.clone(),
        reply_to: None,
        thread_id: None,
        title: None,
    };
    let row = db::messages::create_message(
        &state.db,
        &interaction.channel_id,
        bot,
        Some(&interaction.space_id),
        &message,
        0,
    )
    .await?;
    if loading {
        db::interactions::set_flags(&state.db, &row.id, FLAG_LOADING).await?;
    }
    db::interactions::set_components(&state.db, &row.id, data.components.as_ref()).await?;
    if with_header {
        db::interactions::set_message_interaction(&state.db, &row.id, interaction).await?;
    }
    let row = db::messages::get_message_row(&state.db, &row.id).await?;
    super::messages::apply_mention_counts(state, &row).await;
    let json = message_json(state, row).await?;
    to_space(state, &interaction.space_id, "message.create", json.clone()).await;
    Ok(json)
}

/// An answer only the member who used the command sees: never stored as a
/// message, delivered to that member's sessions alone.
fn ephemeral_json(
    id: &str,
    interaction: &InteractionRow,
    bot: &str,
    data: &ResponseData,
    loading: bool,
) -> Value {
    let flags = FLAG_EPHEMERAL | if loading { FLAG_LOADING } else { 0 };
    serde_json::json!({
        "id": id,
        "channel_id": interaction.channel_id,
        "space_id": interaction.space_id,
        "author_id": bot,
        "content": data.content.clone().unwrap_or_default(),
        "type": "default",
        "timestamp": chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S+00:00").to_string(),
        "edited_at": null,
        "tts": false,
        "pinned": false,
        "mention_everyone": false,
        "mentions": [],
        "mention_roles": [],
        "attachments": [],
        "embeds": data.embeds.clone().unwrap_or_default(),
        "reactions": null,
        "reply_to": null,
        "flags": flags,
        "webhook_id": null,
        "thread_id": null,
        "reply_count": 0,
        "title": null,
        "components": data.components.clone().unwrap_or(Value::Array(Vec::new())),
        "interaction": db::interactions::interaction_summary(interaction),
    })
}

/// Applies `data` to a stored message (content, embeds, components),
/// finishing a "thinking…" placeholder.
async fn update_stored(
    state: &AppState,
    space_id: &str,
    message_id: &str,
    data: &ResponseData,
) -> Result<Value, AppError> {
    let before = db::messages::get_message_row(&state.db, message_id).await?;
    let was_loading = before.flags & FLAG_LOADING != 0;
    if data.content.is_some() || data.embeds.is_some() {
        let update = UpdateMessage {
            content: data.content.clone(),
            embeds: data.embeds.clone(),
            title: None,
        };
        db::messages::update_message(&state.db, message_id, &update, state.db_is_postgres).await?;
    }
    if let Some(ref components) = data.components {
        db::interactions::set_components(&state.db, message_id, Some(components)).await?;
    }
    if was_loading {
        db::interactions::set_flags(&state.db, message_id, before.flags & !FLAG_LOADING).await?;
        db::interactions::clear_edited(&state.db, message_id).await?;
    }
    let row = db::messages::get_message_row(&state.db, message_id).await?;
    let json = message_json(state, row).await?;
    to_space(state, space_id, "message.update", json.clone()).await;
    Ok(json)
}

/// Applies `data` to an ephemeral answer and re-delivers it.
async fn update_ephemeral(
    state: &AppState,
    owner: &InteractionRow,
    data: &ResponseData,
) -> Result<Value, AppError> {
    let mut stored: Value = owner
        .ephemeral_response
        .as_deref()
        .and_then(|raw| serde_json::from_str(raw).ok())
        .ok_or_else(|| AppError::NotFound("unknown_message".into()))?;
    if let Some(ref content) = data.content {
        stored["content"] = Value::String(content.clone());
    }
    if let Some(ref embeds) = data.embeds {
        stored["embeds"] = serde_json::to_value(embeds).unwrap_or_default();
    }
    if let Some(ref components) = data.components {
        stored["components"] = components.clone();
    }
    stored["flags"] =
        Value::from(stored["flags"].as_i64().unwrap_or(FLAG_EPHEMERAL) & !FLAG_LOADING);
    db::interactions::set_ephemeral_response(&state.db, &owner.id, Some(&stored)).await?;
    to_user(state, &owner.user_id, "message.update", stored.clone()).await;
    Ok(stored)
}

/// For a component interaction: `None` when its source message is stored,
/// or the earlier interaction owning the ephemeral message it came from.
async fn source_owner(
    state: &AppState,
    interaction: &InteractionRow,
) -> Result<Option<InteractionRow>, AppError> {
    let Some(ref source) = interaction.source_message_id else {
        return Err(AppError::BadRequest(
            "only a component interaction can update its message".into(),
        ));
    };
    if db::messages::get_message_row(&state.db, source)
        .await
        .is_ok()
    {
        return Ok(None);
    }
    Ok(Some(db::interactions::get(&state.db, source).await?))
}

/// Updates the message a component interaction came from.
async fn update_source(
    state: &AppState,
    interaction: &InteractionRow,
    data: &ResponseData,
) -> Result<Value, AppError> {
    match source_owner(state, interaction).await? {
        None => {
            let source = interaction.source_message_id.clone().unwrap_or_default();
            update_stored(state, &interaction.space_id, &source, data).await
        }
        Some(owner) => update_ephemeral(state, &owner, data).await,
    }
}

/// POST /interactions/{id}/{token}/callback — the application's first answer.
pub async fn interaction_callback(
    state: State<AppState>,
    Path((interaction_id, token)): Path<(String, String)>,
    body: Bytes,
) -> Result<Response, AppError> {
    let interaction = interaction_by_token(&state, &interaction_id, &token).await?;
    let callback: CallbackBody = parse_json(&body)?;
    let kind = callback_kind(&callback.kind)
        .ok_or_else(|| AppError::BadRequest("unknown callback type".into()))?;
    if kind == "pong" {
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    let is_component = interaction.interaction_type == "message_component";
    if matches!(kind, "deferred_update" | "update_message") && !is_component {
        return Err(AppError::BadRequest(
            "only a component interaction can update its message".into(),
        ));
    }
    let data = callback
        .data
        .unwrap_or_default()
        .checked(kind == "message")?;
    if !db::interactions::claim_response(&state.db, &interaction.id).await? {
        return Err(AppError::BadRequest(
            "this interaction has already been answered".into(),
        ));
    }
    let app = db::app_commands::get_application(&state.db, &interaction.application_id).await?;
    let bot = bot_of(&app)?;

    match kind {
        "message" | "deferred_message" => {
            let loading = kind == "deferred_message";
            if data.ephemeral() {
                let json = ephemeral_json(&interaction.id, &interaction, &bot, &data, loading);
                db::interactions::set_ephemeral_response(&state.db, &interaction.id, Some(&json))
                    .await?;
                to_user(&state, &interaction.user_id, "message.create", json).await;
            } else {
                let json = post_answer(&state, &interaction, &bot, &data, loading, true).await?;
                if let Some(id) = json["id"].as_str() {
                    db::interactions::set_response_message(&state.db, &interaction.id, id).await?;
                }
            }
        }
        "update_message" => {
            update_source(&state, &interaction, &data).await?;
        }
        _ => {} // deferred_update: nothing visible until an edit
    }

    to_user(
        &state,
        &interaction.user_id,
        "interaction.success",
        serde_json::json!({ "id": interaction.id }),
    )
    .await;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// PATCH /interactions/{id}/{token}/original — edits the answer (fills in a
/// "thinking…" placeholder), or for a component interaction the message it
/// came from.
pub async fn edit_original(
    state: State<AppState>,
    Path((interaction_id, token)): Path<(String, String)>,
    body: Bytes,
) -> Result<Json<Value>, AppError> {
    let interaction = interaction_by_token(&state, &interaction_id, &token).await?;
    let data: ResponseData = parse_json::<ResponseData>(&body)?.checked(false)?;
    if let Some(ref message_id) = interaction.response_message_id {
        return Ok(Json(
            update_stored(&state, &interaction.space_id, message_id, &data).await?,
        ));
    }
    if interaction.ephemeral_response.is_some() {
        return Ok(Json(update_ephemeral(&state, &interaction, &data).await?));
    }
    if interaction.interaction_type == "message_component" && interaction.responded {
        return Ok(Json(update_source(&state, &interaction, &data).await?));
    }
    Err(AppError::BadRequest(
        "this interaction has no answer to edit yet".into(),
    ))
}

/// DELETE /interactions/{id}/{token}/original
pub async fn delete_original(
    state: State<AppState>,
    Path((interaction_id, token)): Path<(String, String)>,
) -> Result<Response, AppError> {
    let interaction = interaction_by_token(&state, &interaction_id, &token).await?;
    let gone = serde_json::json!({
        "id": interaction.response_message_id.clone().unwrap_or_else(|| interaction.id.clone()),
        "channel_id": interaction.channel_id,
        "space_id": interaction.space_id,
    });
    if let Some(ref message_id) = interaction.response_message_id {
        db::messages::delete_message(&state.db, message_id).await?;
        to_space(&state, &interaction.space_id, "message.delete", gone).await;
    } else if interaction.ephemeral_response.is_some() {
        db::interactions::set_ephemeral_response(&state.db, &interaction.id, None).await?;
        to_user(&state, &interaction.user_id, "message.delete", gone).await;
    } else {
        return Err(AppError::NotFound("unknown_message".into()));
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// POST /interactions/{id}/{token}/followup — another message after the
/// first answer.
pub async fn create_followup(
    state: State<AppState>,
    Path((interaction_id, token)): Path<(String, String)>,
    body: Bytes,
) -> Result<Json<Value>, AppError> {
    let interaction = interaction_by_token(&state, &interaction_id, &token).await?;
    if !interaction.responded {
        return Err(AppError::BadRequest(
            "answer the interaction before following up".into(),
        ));
    }
    let data: ResponseData = parse_json::<ResponseData>(&body)?.checked(true)?;
    let app = db::app_commands::get_application(&state.db, &interaction.application_id).await?;
    let bot = bot_of(&app)?;
    if data.ephemeral() {
        let id = crate::snowflake::generate();
        let json = ephemeral_json(&id, &interaction, &bot, &data, false);
        to_user(&state, &interaction.user_id, "message.create", json.clone()).await;
        return Ok(Json(json));
    }
    Ok(Json(
        post_answer(&state, &interaction, &bot, &data, false, false).await?,
    ))
}

// ── Components on a bot's own messages ──────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ComponentsBody {
    #[serde(default)]
    pub components: Value,
}

/// PUT /channels/{channel_id}/messages/{message_id}/components — a bot sets
/// (or, with `[]`/`null`, removes) the buttons and menus of its message.
pub async fn set_message_components(
    state: State<AppState>,
    Path((channel_id, message_id)): Path<(String, String)>,
    auth: AuthUser,
    Json(input): Json<ComponentsBody>,
) -> Result<Json<Value>, AppError> {
    let row = db::messages::get_message_row(&state.db, &message_id).await?;
    if row.channel_id != channel_id {
        return Err(AppError::NotFound("unknown_message".into()));
    }
    if row.author_id != auth.user_id || !auth.is_bot {
        return Err(AppError::Forbidden(
            "only the bot that posted a message can set its components".into(),
        ));
    }
    let space_id = row
        .space_id
        .clone()
        .ok_or_else(|| AppError::BadRequest("components are only for space messages".into()))?;
    let components = if input.components.is_null() {
        Value::Array(Vec::new())
    } else {
        validate_components(&input.components).map_err(AppError::BadRequest)?
    };
    db::interactions::set_components(&state.db, &message_id, Some(&components)).await?;
    let json = message_json(&state, row).await?;
    to_space(&state, &space_id, "message.update", json.clone()).await;
    Ok(Json(serde_json::json!({ "data": json })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn callback_types_accept_words_and_codes() {
        assert_eq!(callback_kind(&json!(4)), Some("message"));
        assert_eq!(
            callback_kind(&json!("deferred_message")),
            Some("deferred_message")
        );
        assert_eq!(callback_kind(&json!(7)), Some("update_message"));
        assert_eq!(callback_kind(&json!(9)), None);
    }

    #[test]
    fn answers_need_something_to_show() {
        assert!(ResponseData::default().checked(true).is_err());
        assert!(ResponseData::default().checked(false).is_ok());
        let ephemeral = ResponseData {
            content: Some("hi".into()),
            flags: Some(64),
            ..Default::default()
        };
        assert!(ephemeral.ephemeral());
        let too_long = ResponseData {
            content: Some("x".repeat(4001)),
            ..Default::default()
        };
        assert!(too_long.checked(true).is_err());
    }

    #[test]
    fn tokens_compare_exactly() {
        assert!(token_eq("abc", "abc"));
        assert!(!token_eq("abc", "abd"));
        assert!(!token_eq("abc", "ab"));
    }
}
