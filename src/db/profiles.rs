use sqlx::{AnyPool, Row};

use crate::error::AppError;

/// The extras of a user's profile.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct ProfileExtras {
    pub pronouns: Option<String>,
    pub theme_primary: Option<i64>,
    pub theme_accent: Option<i64>,
}

pub async fn get(pool: &AnyPool, user_id: &str) -> Result<ProfileExtras, AppError> {
    let row = sqlx::query(&super::q(
        "SELECT pronouns, theme_primary, theme_accent FROM user_profiles WHERE user_id = ?",
    ))
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    Ok(row
        .map(|row| ProfileExtras {
            pronouns: row.get("pronouns"),
            theme_primary: row.get("theme_primary"),
            theme_accent: row.get("theme_accent"),
        })
        .unwrap_or_default())
}

pub async fn set(pool: &AnyPool, user_id: &str, extras: &ProfileExtras) -> Result<(), AppError> {
    sqlx::query(&super::q(
        "INSERT INTO user_profiles (user_id, pronouns, theme_primary, theme_accent) VALUES (?, ?, ?, ?) \
         ON CONFLICT (user_id) DO UPDATE SET pronouns = excluded.pronouns, \
         theme_primary = excluded.theme_primary, theme_accent = excluded.theme_accent",
    ))
    .bind(user_id)
    .bind(&extras.pronouns)
    .bind(extras.theme_primary)
    .bind(extras.theme_accent)
    .execute(pool)
    .await?;
    Ok(())
}

/// The note `owner_id` keeps about `target_id`, if any.
pub async fn note(
    pool: &AnyPool,
    owner_id: &str,
    target_id: &str,
) -> Result<Option<String>, AppError> {
    Ok(sqlx::query_scalar(&super::q(
        "SELECT note FROM user_notes WHERE owner_id = ? AND target_id = ?",
    ))
    .bind(owner_id)
    .bind(target_id)
    .fetch_optional(pool)
    .await?)
}

/// Saves the note, or removes it when `note` is empty.
pub async fn set_note(
    pool: &AnyPool,
    owner_id: &str,
    target_id: &str,
    note: &str,
) -> Result<(), AppError> {
    if note.is_empty() {
        sqlx::query(&super::q(
            "DELETE FROM user_notes WHERE owner_id = ? AND target_id = ?",
        ))
        .bind(owner_id)
        .bind(target_id)
        .execute(pool)
        .await?;
        return Ok(());
    }
    let now = chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S+00:00")
        .to_string();
    sqlx::query(&super::q(
        "INSERT INTO user_notes (owner_id, target_id, note, updated_at) VALUES (?, ?, ?, ?) \
         ON CONFLICT (owner_id, target_id) DO UPDATE SET note = excluded.note, updated_at = excluded.updated_at",
    ))
    .bind(owner_id)
    .bind(target_id)
    .bind(note)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// The ids of the users `owner_id` has notes about, with the notes.
pub async fn notes_of(pool: &AnyPool, owner_id: &str) -> Result<Vec<(String, String)>, AppError> {
    let rows = sqlx::query(&super::q(
        "SELECT target_id, note FROM user_notes WHERE owner_id = ? ORDER BY updated_at DESC",
    ))
    .bind(owner_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| (row.get("target_id"), row.get("note")))
        .collect())
}
