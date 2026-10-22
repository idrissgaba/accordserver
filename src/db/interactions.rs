use std::collections::HashMap;

use sqlx::{AnyPool, Row};

use crate::error::AppError;
use crate::snowflake;

/// How long an application may answer an interaction.
pub const INTERACTION_TTL_MINUTES: i64 = 15;

/// Message flag: only the member who used the command sees it.
pub const FLAG_EPHEMERAL: i64 = 1 << 6;

/// Message flag: the application is still preparing its answer.
pub const FLAG_LOADING: i64 = 1 << 7;

#[derive(Debug, Clone)]
pub struct InteractionRow {
    pub id: String,
    pub token: String,
    pub application_id: String,
    pub interaction_type: String,
    pub user_id: String,
    pub space_id: String,
    pub channel_id: String,
    pub name: Option<String>,
    pub source_message_id: Option<String>,
    pub response_message_id: Option<String>,
    pub ephemeral_response: Option<String>,
    pub responded: bool,
    pub created_at: String,
}

pub struct NewInteraction<'a> {
    pub application_id: &'a str,
    pub interaction_type: &'a str,
    pub user_id: &'a str,
    pub space_id: &'a str,
    pub channel_id: &'a str,
    pub name: Option<&'a str>,
    pub source_message_id: Option<&'a str>,
}

fn now_string() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S+00:00")
        .to_string()
}

fn generate_token() -> String {
    use rand::Rng;
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::thread_rng();
    (0..64)
        .map(|_| CHARSET[rng.gen_range(0..CHARSET.len())] as char)
        .collect()
}

fn row_to_interaction(row: sqlx::any::AnyRow) -> InteractionRow {
    InteractionRow {
        id: row.get("id"),
        token: row.get("token"),
        application_id: row.get("application_id"),
        interaction_type: row.get("type"),
        user_id: row.get("user_id"),
        space_id: row.get("space_id"),
        channel_id: row.get("channel_id"),
        name: row.get("name"),
        source_message_id: row.get("source_message_id"),
        response_message_id: row.get("response_message_id"),
        ephemeral_response: row.get("ephemeral_response"),
        responded: crate::db::get_bool(&row, "responded"),
        created_at: row.get("created_at"),
    }
}

/// Records a new interaction, clearing out expired ones on the way.
pub async fn create(pool: &AnyPool, new: &NewInteraction<'_>) -> Result<InteractionRow, AppError> {
    let cutoff = (chrono::Utc::now() - chrono::Duration::days(1))
        .format("%Y-%m-%dT%H:%M:%S+00:00")
        .to_string();
    sqlx::query(&super::q("DELETE FROM interactions WHERE created_at < ?"))
        .bind(cutoff)
        .execute(pool)
        .await?;
    let id = snowflake::generate();
    sqlx::query(&super::q(
        "INSERT INTO interactions (id, token, application_id, type, user_id, space_id, channel_id, name, source_message_id, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    ))
    .bind(&id)
    .bind(generate_token())
    .bind(new.application_id)
    .bind(new.interaction_type)
    .bind(new.user_id)
    .bind(new.space_id)
    .bind(new.channel_id)
    .bind(new.name)
    .bind(new.source_message_id)
    .bind(now_string())
    .execute(pool)
    .await?;
    get(pool, &id).await
}

pub async fn get(pool: &AnyPool, id: &str) -> Result<InteractionRow, AppError> {
    sqlx::query(&super::q(
        "SELECT id, token, application_id, type, user_id, space_id, channel_id, name, source_message_id, response_message_id, ephemeral_response, responded, created_at FROM interactions WHERE id = ?",
    ))
    .bind(id)
    .fetch_optional(pool)
    .await?
    .map(row_to_interaction)
    .ok_or_else(|| AppError::NotFound("unknown interaction".into()))
}

/// Whether `row` can still be answered at `now`.
pub fn is_live(row: &InteractionRow, now: chrono::DateTime<chrono::Utc>) -> bool {
    chrono::DateTime::parse_from_rfc3339(&row.created_at)
        .map(|created| {
            now.signed_duration_since(created.with_timezone(&chrono::Utc))
                < chrono::Duration::minutes(INTERACTION_TTL_MINUTES)
        })
        .unwrap_or(false)
}

/// Claims the single initial answer. False when it was already given.
pub async fn claim_response(pool: &AnyPool, id: &str) -> Result<bool, AppError> {
    let claimed = sqlx::query(&super::q(
        "UPDATE interactions SET responded = 1 WHERE id = ? AND responded = 0",
    ))
    .bind(id)
    .execute(pool)
    .await?;
    Ok(claimed.rows_affected() == 1)
}

pub async fn set_response_message(
    pool: &AnyPool,
    id: &str,
    message_id: &str,
) -> Result<(), AppError> {
    sqlx::query(&super::q(
        "UPDATE interactions SET response_message_id = ? WHERE id = ?",
    ))
    .bind(message_id)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_ephemeral_response(
    pool: &AnyPool,
    id: &str,
    message: Option<&serde_json::Value>,
) -> Result<(), AppError> {
    sqlx::query(&super::q(
        "UPDATE interactions SET ephemeral_response = ? WHERE id = ?",
    ))
    .bind(message.map(|m| m.to_string()))
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Stores (or, with `None`, removes) the components of a message.
pub async fn set_components(
    pool: &AnyPool,
    message_id: &str,
    components: Option<&serde_json::Value>,
) -> Result<(), AppError> {
    sqlx::query(&super::q(
        "DELETE FROM message_components WHERE message_id = ?",
    ))
    .bind(message_id)
    .execute(pool)
    .await?;
    if let Some(components) = components.filter(|c| c.as_array().is_some_and(|a| !a.is_empty())) {
        sqlx::query(&super::q(
            "INSERT INTO message_components (message_id, components) VALUES (?, ?)",
        ))
        .bind(message_id)
        .bind(components.to_string())
        .execute(pool)
        .await?;
    }
    Ok(())
}

pub async fn get_components(
    pool: &AnyPool,
    message_id: &str,
) -> Result<Option<serde_json::Value>, AppError> {
    let raw: Option<String> = sqlx::query_scalar(&super::q(
        "SELECT components FROM message_components WHERE message_id = ?",
    ))
    .bind(message_id)
    .fetch_optional(pool)
    .await?;
    Ok(raw.and_then(|r| serde_json::from_str(&r).ok()))
}

/// Records which interaction a bot's message answers.
pub async fn set_message_interaction(
    pool: &AnyPool,
    message_id: &str,
    interaction: &InteractionRow,
) -> Result<(), AppError> {
    sqlx::query(&super::q(
        "INSERT INTO message_interactions (message_id, interaction_id, type, name, user_id) VALUES (?, ?, ?, ?, ?)",
    ))
    .bind(message_id)
    .bind(&interaction.id)
    .bind(&interaction.interaction_type)
    .bind(&interaction.name)
    .bind(&interaction.user_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// The `interaction` summary a message carries ("X used /play").
pub fn interaction_summary(row: &InteractionRow) -> serde_json::Value {
    serde_json::json!({
        "id": row.id,
        "type": row.interaction_type,
        "name": row.name,
        "user_id": row.user_id,
    })
}

pub async fn set_flags(pool: &AnyPool, message_id: &str, flags: i64) -> Result<(), AppError> {
    sqlx::query(&super::q("UPDATE messages SET flags = ? WHERE id = ?"))
        .bind(flags)
        .bind(message_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// A deferred answer filled in for the first time is not an edit.
pub async fn clear_edited(pool: &AnyPool, message_id: &str) -> Result<(), AppError> {
    sqlx::query(&super::q(
        "UPDATE messages SET edited_at = NULL WHERE id = ?",
    ))
    .bind(message_id)
    .execute(pool)
    .await?;
    Ok(())
}

fn placeholders(n: usize) -> String {
    vec!["?"; n].join(", ")
}

/// Components of the messages among `ids` that have any.
pub async fn components_for_messages(
    pool: &AnyPool,
    ids: &[String],
) -> Result<HashMap<String, serde_json::Value>, AppError> {
    let mut out = HashMap::new();
    if ids.is_empty() {
        return Ok(out);
    }
    let sql = super::q(&format!(
        "SELECT message_id, components FROM message_components WHERE message_id IN ({})",
        placeholders(ids.len())
    ));
    let mut query = sqlx::query(&sql);
    for id in ids {
        query = query.bind(id);
    }
    for row in query.fetch_all(pool).await? {
        let raw: String = row.get("components");
        if let Ok(value) = serde_json::from_str(&raw) {
            out.insert(row.get("message_id"), value);
        }
    }
    Ok(out)
}

/// Interaction summaries of the messages among `ids` that answer one.
pub async fn interactions_for_messages(
    pool: &AnyPool,
    ids: &[String],
) -> Result<HashMap<String, serde_json::Value>, AppError> {
    let mut out = HashMap::new();
    if ids.is_empty() {
        return Ok(out);
    }
    let sql = super::q(&format!(
        "SELECT message_id, interaction_id, type, name, user_id FROM message_interactions WHERE message_id IN ({})",
        placeholders(ids.len())
    ));
    let mut query = sqlx::query(&sql);
    for id in ids {
        query = query.bind(id);
    }
    for row in query.fetch_all(pool).await? {
        let message_id: String = row.get("message_id");
        let interaction_id: String = row.get("interaction_id");
        let kind: String = row.get("type");
        let name: Option<String> = row.get("name");
        let user_id: String = row.get("user_id");
        out.insert(
            message_id,
            serde_json::json!({
                "id": interaction_id,
                "type": kind,
                "name": name,
                "user_id": user_id,
            }),
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(created_at: &str) -> InteractionRow {
        InteractionRow {
            id: "1".into(),
            token: "t".into(),
            application_id: "a".into(),
            interaction_type: "application_command".into(),
            user_id: "u".into(),
            space_id: "s".into(),
            channel_id: "c".into(),
            name: Some("play".into()),
            source_message_id: None,
            response_message_id: None,
            ephemeral_response: None,
            responded: false,
            created_at: created_at.into(),
        }
    }

    #[test]
    fn interactions_expire_after_fifteen_minutes() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-10-22T12:00:00+00:00")
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert!(is_live(&row("2026-10-22T11:50:00+00:00"), now));
        assert!(!is_live(&row("2026-10-22T11:44:59+00:00"), now));
        assert!(!is_live(&row("garbage"), now));
    }
}
