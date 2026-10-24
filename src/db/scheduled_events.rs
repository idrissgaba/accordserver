use std::collections::{HashMap, HashSet};

use sqlx::{AnyPool, Row};

use crate::error::AppError;
use crate::models::scheduled_event::{EventFields, ScheduledEvent};
use crate::snowflake;

const SELECT_EVENTS: &str = "SELECT id, space_id, channel_id, creator_id, name, description, location, entity_type, status, start_time, end_time, image, created_at FROM scheduled_events";

fn row_to_event(row: sqlx::any::AnyRow) -> ScheduledEvent {
    ScheduledEvent {
        id: row.get("id"),
        space_id: row.get("space_id"),
        channel_id: row.get("channel_id"),
        creator_id: row.get("creator_id"),
        name: row.get("name"),
        description: row.get("description"),
        location: row.get("location"),
        entity_type: row.get("entity_type"),
        status: row.get("status"),
        start_time: row.get("start_time"),
        end_time: row.get("end_time"),
        image: row.get("image"),
        created_at: row.get("created_at"),
    }
}

pub async fn create(
    pool: &AnyPool,
    space_id: &str,
    creator_id: &str,
    fields: &EventFields,
) -> Result<ScheduledEvent, AppError> {
    let id = snowflake::generate();
    sqlx::query(&super::q(
        "INSERT INTO scheduled_events (id, space_id, channel_id, creator_id, name, description, location, entity_type, start_time, end_time) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    ))
    .bind(&id)
    .bind(space_id)
    .bind(&fields.channel_id)
    .bind(creator_id)
    .bind(&fields.name)
    .bind(&fields.description)
    .bind(&fields.location)
    .bind(&fields.entity_type)
    .bind(&fields.start_time)
    .bind(&fields.end_time)
    .execute(pool)
    .await?;
    get(pool, &id).await
}

pub async fn get(pool: &AnyPool, id: &str) -> Result<ScheduledEvent, AppError> {
    sqlx::query(&super::q(&format!("{SELECT_EVENTS} WHERE id = ?")))
        .bind(id)
        .fetch_optional(pool)
        .await?
        .map(row_to_event)
        .ok_or_else(|| AppError::NotFound("unknown event".into()))
}

/// The space's events that are coming up or under way, soonest first.
pub async fn list_open(pool: &AnyPool, space_id: &str) -> Result<Vec<ScheduledEvent>, AppError> {
    let rows = sqlx::query(&super::q(&format!(
        "{SELECT_EVENTS} WHERE space_id = ? AND status IN ('scheduled', 'active') ORDER BY start_time, id"
    )))
    .bind(space_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(row_to_event).collect())
}

pub async fn update_fields(pool: &AnyPool, id: &str, fields: &EventFields) -> Result<(), AppError> {
    sqlx::query(&super::q(
        "UPDATE scheduled_events SET name = ?, description = ?, entity_type = ?, channel_id = ?, location = ?, start_time = ?, end_time = ? WHERE id = ?",
    ))
    .bind(&fields.name)
    .bind(&fields.description)
    .bind(&fields.entity_type)
    .bind(&fields.channel_id)
    .bind(&fields.location)
    .bind(&fields.start_time)
    .bind(&fields.end_time)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Moves the event from `from` to `to`; false when it was no longer in `from`
/// (someone, or the scheduler, got there first).
pub async fn set_status(pool: &AnyPool, id: &str, from: &str, to: &str) -> Result<bool, AppError> {
    let done = sqlx::query(&super::q(
        "UPDATE scheduled_events SET status = ? WHERE id = ? AND status = ?",
    ))
    .bind(to)
    .bind(id)
    .bind(from)
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

pub async fn set_image(pool: &AnyPool, id: &str, image: Option<&str>) -> Result<(), AppError> {
    sqlx::query(&super::q(
        "UPDATE scheduled_events SET image = ? WHERE id = ?",
    ))
    .bind(image)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn delete(pool: &AnyPool, id: &str) -> Result<(), AppError> {
    sqlx::query(&super::q("DELETE FROM scheduled_events WHERE id = ?"))
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Marks `user_id` interested; false when they already were.
pub async fn add_user(pool: &AnyPool, event_id: &str, user_id: &str) -> Result<bool, AppError> {
    let done = sqlx::query(&super::q(
        "INSERT INTO scheduled_event_users (event_id, user_id) VALUES (?, ?) ON CONFLICT (event_id, user_id) DO NOTHING",
    ))
    .bind(event_id)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

pub async fn remove_user(pool: &AnyPool, event_id: &str, user_id: &str) -> Result<bool, AppError> {
    let done = sqlx::query(&super::q(
        "DELETE FROM scheduled_event_users WHERE event_id = ? AND user_id = ?",
    ))
    .bind(event_id)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

pub async fn users(pool: &AnyPool, event_id: &str) -> Result<Vec<String>, AppError> {
    Ok(sqlx::query_scalar(&super::q(
        "SELECT user_id FROM scheduled_event_users WHERE event_id = ? ORDER BY created_at, user_id",
    ))
    .bind(event_id)
    .fetch_all(pool)
    .await?)
}

/// Interested counts of `ids`, and which of them `user_id` is interested in.
pub async fn interest(
    pool: &AnyPool,
    ids: &[String],
    user_id: &str,
) -> Result<(HashMap<String, i64>, HashSet<String>), AppError> {
    let mut counts = HashMap::new();
    let mut mine = HashSet::new();
    if ids.is_empty() {
        return Ok((counts, mine));
    }
    let placeholders = vec!["?"; ids.len()].join(", ");
    let sql = super::q(&format!(
        "SELECT event_id, user_id FROM scheduled_event_users WHERE event_id IN ({placeholders})"
    ));
    let mut query = sqlx::query(&sql);
    for id in ids {
        query = query.bind(id);
    }
    for row in query.fetch_all(pool).await? {
        let event_id: String = row.get("event_id");
        let who: String = row.get("user_id");
        if who == user_id {
            mine.insert(event_id.clone());
        }
        *counts.entry(event_id).or_insert(0) += 1;
    }
    Ok((counts, mine))
}

/// Scheduled events whose start has come.
pub async fn due_to_start(pool: &AnyPool, now: &str) -> Result<Vec<ScheduledEvent>, AppError> {
    let rows = sqlx::query(&super::q(&format!(
        "{SELECT_EVENTS} WHERE status = 'scheduled' AND start_time <= ?"
    )))
    .bind(now)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(row_to_event).collect())
}

/// Active events (the caller decides which have run their course).
pub async fn active(pool: &AnyPool) -> Result<Vec<ScheduledEvent>, AppError> {
    let rows = sqlx::query(&super::q(&format!(
        "{SELECT_EVENTS} WHERE status = 'active'"
    )))
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(row_to_event).collect())
}
