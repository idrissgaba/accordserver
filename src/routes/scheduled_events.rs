//! Scheduled events: members with `manage_events` plan events in a voice
//! channel or somewhere else; members mark themselves interested. Events start
//! on their own at their start time and end at their end time (or after
//! [`OPEN_ENDED_HOURS`] without one); managers can also start, end or cancel
//! them by hand.

use axum::extract::{Path, State};
use axum::Json;
use serde_json::Value;

use crate::db;
use crate::error::AppError;
use crate::gateway::events::GatewayBroadcast;
use crate::middleware::auth::AuthUser;
use crate::middleware::permissions::{require_membership, require_permission};
use crate::models::scheduled_event::{
    canonical_time, parse_time, transition_allowed, CreateEvent, EventFields, ScheduledEvent,
    UpdateEvent, ENTITY_VOICE, OPEN_ENDED_HOURS, STATUS_ACTIVE, STATUS_COMPLETED, STATUS_SCHEDULED,
};
use crate::state::AppState;
use crate::storage;

/// Most open (scheduled or active) events per space.
const MAX_OPEN_EVENTS: usize = 100;

fn now_string() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S+00:00")
        .to_string()
}

fn event_json(event: &ScheduledEvent, user_count: i64, interested: Option<bool>) -> Value {
    let mut json = serde_json::to_value(event).unwrap_or(Value::Null);
    json["user_count"] = Value::from(user_count);
    if let Some(interested) = interested {
        json["interested"] = Value::Bool(interested);
    }
    json
}

async fn broadcast(state: &AppState, space_id: &str, event_type: &str, data: Value) {
    if let Some(ref tx) = *state.gateway_tx.read().await {
        let _ = tx.send(GatewayBroadcast {
            space_id: Some(space_id.to_string()),
            target_user_ids: None,
            event: serde_json::json!({ "op": 0, "type": event_type, "data": data }),
            intent: "scheduled_events".to_string(),
            required_permission: None,
        });
    }
}

/// Announces an event's new state with its interested count.
async fn announce(state: &AppState, event: &ScheduledEvent, event_type: &str) {
    let count = db::scheduled_events::users(&state.db, &event.id)
        .await
        .map(|u| u.len() as i64)
        .unwrap_or(0);
    broadcast(
        state,
        &event.space_id,
        event_type,
        event_json(event, count, None),
    )
    .await;
}

async fn with_interest(
    state: &AppState,
    events: &[ScheduledEvent],
    user_id: &str,
) -> Result<Vec<Value>, AppError> {
    let ids: Vec<String> = events.iter().map(|e| e.id.clone()).collect();
    let (counts, mine) = db::scheduled_events::interest(&state.db, &ids, user_id).await?;
    Ok(events
        .iter()
        .map(|e| {
            event_json(
                e,
                counts.get(&e.id).copied().unwrap_or(0),
                Some(mine.contains(&e.id)),
            )
        })
        .collect())
}

/// Checks that a voice event's channel is a voice channel of the space.
async fn check_channel(
    state: &AppState,
    space_id: &str,
    fields: &EventFields,
) -> Result<(), AppError> {
    if fields.entity_type != ENTITY_VOICE {
        return Ok(());
    }
    let channel_id = fields.channel_id.as_deref().unwrap_or_default();
    let channel = db::channels::get_channel_row(&state.db, channel_id)
        .await
        .map_err(|_| AppError::BadRequest("unknown channel".into()))?;
    if channel.space_id.as_deref() != Some(space_id) || channel.channel_type != "voice" {
        return Err(AppError::BadRequest(
            "a voice event takes place in a voice channel of this space".into(),
        ));
    }
    Ok(())
}

async fn apply_image(
    state: &AppState,
    event: &ScheduledEvent,
    actor: &str,
    image: &str,
) -> Result<(), AppError> {
    let entity = format!("event-{}", event.id);
    if image.is_empty() {
        storage::delete_avatar(&state.storage_path, "banners", &entity).await?;
        return db::scheduled_events::set_image(&state.db, &event.id, None).await;
    }
    if !image.starts_with("data:") {
        return Err(AppError::BadRequest("image must be a data: URI".into()));
    }
    let max = state.settings.load().max_avatar_size as usize;
    let (url, _, _, _) = storage::save_avatar_image(
        state,
        actor,
        Some(&event.space_id),
        "banners",
        &entity,
        image,
        max,
    )
    .await?;
    db::scheduled_events::set_image(&state.db, &event.id, Some(&url)).await
}

/// The event `event_id` of `space_id`.
async fn space_event(
    state: &AppState,
    space_id: &str,
    event_id: &str,
) -> Result<ScheduledEvent, AppError> {
    let event = db::scheduled_events::get(&state.db, event_id).await?;
    if event.space_id != space_id {
        return Err(AppError::NotFound("unknown event".into()));
    }
    Ok(event)
}

/// GET /spaces/{space_id}/events — coming up or under way, soonest first.
pub async fn list_events(
    state: State<AppState>,
    Path(space_id): Path<String>,
    auth: AuthUser,
) -> Result<Json<Value>, AppError> {
    require_membership(&state.db, &space_id, &auth.user_id).await?;
    let events = db::scheduled_events::list_open(&state.db, &space_id).await?;
    Ok(Json(
        serde_json::json!({ "data": with_interest(&state, &events, &auth.user_id).await? }),
    ))
}

/// GET /spaces/{space_id}/events/{event_id}
pub async fn get_event(
    state: State<AppState>,
    Path((space_id, event_id)): Path<(String, String)>,
    auth: AuthUser,
) -> Result<Json<Value>, AppError> {
    require_membership(&state.db, &space_id, &auth.user_id).await?;
    let event = space_event(&state, &space_id, &event_id).await?;
    let json = with_interest(&state, &[event], &auth.user_id).await?;
    Ok(Json(serde_json::json!({ "data": json[0] })))
}

/// POST /spaces/{space_id}/events
pub async fn create_event(
    state: State<AppState>,
    Path(space_id): Path<String>,
    auth: AuthUser,
    Json(input): Json<CreateEvent>,
) -> Result<Json<Value>, AppError> {
    require_permission(&state.db, &space_id, &auth, "manage_events").await?;
    let fields = EventFields {
        name: input.name,
        description: input.description,
        entity_type: input.entity_type,
        channel_id: input.channel_id,
        location: input.location,
        start_time: input.start_time,
        end_time: input.end_time,
    }
    .validate()
    .map_err(AppError::BadRequest)?;
    // A little slack for clocks and slow forms.
    let earliest = chrono::Utc::now() - chrono::Duration::minutes(5);
    if parse_time(&fields.start_time).is_some_and(|t| t < earliest) {
        return Err(AppError::BadRequest(
            "an event can't start in the past".into(),
        ));
    }
    check_channel(&state, &space_id, &fields).await?;
    if db::scheduled_events::list_open(&state.db, &space_id)
        .await?
        .len()
        >= MAX_OPEN_EVENTS
    {
        return Err(AppError::BadRequest(format!(
            "a space has at most {MAX_OPEN_EVENTS} upcoming events"
        )));
    }
    let event = db::scheduled_events::create(&state.db, &space_id, &auth.user_id, &fields).await?;
    if let Some(image) = input.image.as_deref().filter(|i| !i.is_empty()) {
        if let Err(err) = apply_image(&state, &event, &auth.user_id, image).await {
            db::scheduled_events::delete(&state.db, &event.id).await?;
            return Err(err);
        }
    }
    // The creator is interested in their own event.
    db::scheduled_events::add_user(&state.db, &event.id, &auth.user_id).await?;
    let event = db::scheduled_events::get(&state.db, &event.id).await?;
    announce(&state, &event, "scheduled_event.create").await;
    let json = with_interest(&state, &[event], &auth.user_id).await?;
    Ok(Json(serde_json::json!({ "data": json[0] })))
}

/// PATCH /spaces/{space_id}/events/{event_id} — edit, or start / end /
/// cancel with `status`.
pub async fn update_event(
    state: State<AppState>,
    Path((space_id, event_id)): Path<(String, String)>,
    auth: AuthUser,
    Json(input): Json<UpdateEvent>,
) -> Result<Json<Value>, AppError> {
    require_permission(&state.db, &space_id, &auth, "manage_events").await?;
    let event = space_event(&state, &space_id, &event_id).await?;
    let editing = input.name.is_some()
        || input.description.is_some()
        || input.entity_type.is_some()
        || input.channel_id.is_some()
        || input.location.is_some()
        || input.start_time.is_some()
        || input.end_time.is_some();
    if editing {
        if event.status != STATUS_SCHEDULED && event.status != STATUS_ACTIVE {
            return Err(AppError::BadRequest("that event is over".into()));
        }
        let start_changed = input
            .start_time
            .as_deref()
            .map(canonical_time)
            .transpose()
            .map_err(AppError::BadRequest)?
            .is_some_and(|t| t != event.start_time);
        if start_changed && event.status == STATUS_ACTIVE {
            return Err(AppError::BadRequest(
                "an event under way can't be rescheduled".into(),
            ));
        }
        let fields = EventFields {
            name: input.name.unwrap_or(event.name.clone()),
            description: input.description.or(event.description.clone()),
            entity_type: input.entity_type.unwrap_or(event.entity_type.clone()),
            channel_id: input.channel_id.or(event.channel_id.clone()),
            location: input.location.or(event.location.clone()),
            start_time: input.start_time.unwrap_or(event.start_time.clone()),
            end_time: input.end_time.or(event.end_time.clone()),
        }
        .validate()
        .map_err(AppError::BadRequest)?;
        check_channel(&state, &space_id, &fields).await?;
        db::scheduled_events::update_fields(&state.db, &event.id, &fields).await?;
    }
    if let Some(ref image) = input.image {
        apply_image(&state, &event, &auth.user_id, image).await?;
    }
    if let Some(ref status) = input.status {
        if status != &event.status {
            if !transition_allowed(&event.status, status) {
                return Err(AppError::BadRequest(format!(
                    "an event can't go from {} to {status}",
                    event.status
                )));
            }
            if !db::scheduled_events::set_status(&state.db, &event.id, &event.status, status)
                .await?
            {
                return Err(AppError::BadRequest(
                    "that event changed in the meantime".into(),
                ));
            }
        }
    }
    let event = db::scheduled_events::get(&state.db, &event.id).await?;
    announce(&state, &event, "scheduled_event.update").await;
    let json = with_interest(&state, &[event], &auth.user_id).await?;
    Ok(Json(serde_json::json!({ "data": json[0] })))
}

/// DELETE /spaces/{space_id}/events/{event_id}
pub async fn delete_event(
    state: State<AppState>,
    Path((space_id, event_id)): Path<(String, String)>,
    auth: AuthUser,
) -> Result<Json<Value>, AppError> {
    require_permission(&state.db, &space_id, &auth, "manage_events").await?;
    let event = space_event(&state, &space_id, &event_id).await?;
    db::scheduled_events::delete(&state.db, &event.id).await?;
    let _ = storage::delete_avatar(
        &state.storage_path,
        "banners",
        &format!("event-{}", event.id),
    )
    .await;
    broadcast(
        &state,
        &space_id,
        "scheduled_event.delete",
        serde_json::json!({ "id": event.id, "space_id": space_id }),
    )
    .await;
    Ok(Json(serde_json::json!({ "data": null })))
}

/// GET /spaces/{space_id}/events/{event_id}/users — who's interested.
pub async fn list_event_users(
    state: State<AppState>,
    Path((space_id, event_id)): Path<(String, String)>,
    auth: AuthUser,
) -> Result<Json<Value>, AppError> {
    require_membership(&state.db, &space_id, &auth.user_id).await?;
    let event = space_event(&state, &space_id, &event_id).await?;
    let users = db::scheduled_events::users(&state.db, &event.id).await?;
    Ok(Json(serde_json::json!({ "data": users })))
}

async fn interest_changed(
    state: &AppState,
    event: &ScheduledEvent,
    user_id: &str,
    event_type: &str,
) -> Result<Json<Value>, AppError> {
    broadcast(
        state,
        &event.space_id,
        event_type,
        serde_json::json!({
            "event_id": event.id,
            "user_id": user_id,
            "space_id": event.space_id,
        }),
    )
    .await;
    let json = with_interest(state, std::slice::from_ref(event), user_id).await?;
    Ok(Json(serde_json::json!({ "data": json[0] })))
}

/// PUT /spaces/{space_id}/events/{event_id}/users/@me — interested.
pub async fn add_interest(
    state: State<AppState>,
    Path((space_id, event_id)): Path<(String, String)>,
    auth: AuthUser,
) -> Result<Json<Value>, AppError> {
    require_membership(&state.db, &space_id, &auth.user_id).await?;
    let event = space_event(&state, &space_id, &event_id).await?;
    if event.status != STATUS_SCHEDULED && event.status != STATUS_ACTIVE {
        return Err(AppError::BadRequest("that event is over".into()));
    }
    if db::scheduled_events::add_user(&state.db, &event.id, &auth.user_id).await? {
        return interest_changed(&state, &event, &auth.user_id, "scheduled_event.user_add").await;
    }
    let json = with_interest(&state, &[event], &auth.user_id).await?;
    Ok(Json(serde_json::json!({ "data": json[0] })))
}

/// DELETE /spaces/{space_id}/events/{event_id}/users/@me
pub async fn remove_interest(
    state: State<AppState>,
    Path((space_id, event_id)): Path<(String, String)>,
    auth: AuthUser,
) -> Result<Json<Value>, AppError> {
    require_membership(&state.db, &space_id, &auth.user_id).await?;
    let event = space_event(&state, &space_id, &event_id).await?;
    if db::scheduled_events::remove_user(&state.db, &event.id, &auth.user_id).await? {
        return interest_changed(&state, &event, &auth.user_id, "scheduled_event.user_remove")
            .await;
    }
    let json = with_interest(&state, &[event], &auth.user_id).await?;
    Ok(Json(serde_json::json!({ "data": json[0] })))
}

/// When an active event is over: its end time, or [`OPEN_ENDED_HOURS`] after
/// its start without one.
fn ends_at(event: &ScheduledEvent) -> Option<chrono::DateTime<chrono::Utc>> {
    match event.end_time.as_deref() {
        Some(end) => parse_time(end),
        None => {
            parse_time(&event.start_time).map(|s| s + chrono::Duration::hours(OPEN_ENDED_HOURS))
        }
    }
}

/// Starts events whose time has come and ends those that are over. Called
/// periodically.
pub async fn tick(state: &AppState) {
    let now = chrono::Utc::now();
    let due = db::scheduled_events::due_to_start(&state.db, &now_string())
        .await
        .unwrap_or_default();
    for event in due {
        if let Ok(true) =
            db::scheduled_events::set_status(&state.db, &event.id, STATUS_SCHEDULED, STATUS_ACTIVE)
                .await
        {
            if let Ok(event) = db::scheduled_events::get(&state.db, &event.id).await {
                announce(state, &event, "scheduled_event.update").await;
            }
        }
    }
    for event in db::scheduled_events::active(&state.db)
        .await
        .unwrap_or_default()
    {
        if ends_at(&event).is_some_and(|end| end <= now) {
            if let Ok(true) = db::scheduled_events::set_status(
                &state.db,
                &event.id,
                STATUS_ACTIVE,
                STATUS_COMPLETED,
            )
            .await
            {
                if let Ok(event) = db::scheduled_events::get(&state.db, &event.id).await {
                    announce(state, &event, "scheduled_event.update").await;
                }
            }
        }
    }
}
