use std::collections::HashMap;

use serde::Serialize;
use sqlx::{AnyPool, Row};

use crate::error::AppError;

#[derive(Debug, Clone, Serialize)]
pub struct Sticker {
    pub id: String,
    pub space_id: String,
    pub creator_id: Option<String>,
    pub name: String,
    pub description: Option<String>,
    /// An emoji the sticker relates to (used for suggestions).
    pub tags: Option<String>,
    /// CDN path of the image.
    pub image: String,
    /// `png`, `gif` or `webp`.
    pub format: String,
    pub created_at: String,
}

const SELECT_STICKERS: &str =
    "SELECT id, space_id, creator_id, name, description, tags, image, format, created_at FROM stickers";

fn row_to_sticker(row: sqlx::any::AnyRow) -> Sticker {
    Sticker {
        id: row.get("id"),
        space_id: row.get("space_id"),
        creator_id: row.get("creator_id"),
        name: row.get("name"),
        description: row.get("description"),
        tags: row.get("tags"),
        image: row.get("image"),
        format: row.get("format"),
        created_at: row.get("created_at"),
    }
}

pub struct NewSticker<'a> {
    pub id: &'a str,
    pub space_id: &'a str,
    pub creator_id: &'a str,
    pub name: &'a str,
    pub description: Option<&'a str>,
    pub tags: Option<&'a str>,
    pub image: &'a str,
    pub format: &'a str,
}

pub async fn create(pool: &AnyPool, new: &NewSticker<'_>) -> Result<Sticker, AppError> {
    sqlx::query(&super::q(
        "INSERT INTO stickers (id, space_id, creator_id, name, description, tags, image, format) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    ))
    .bind(new.id)
    .bind(new.space_id)
    .bind(new.creator_id)
    .bind(new.name)
    .bind(new.description)
    .bind(new.tags)
    .bind(new.image)
    .bind(new.format)
    .execute(pool)
    .await?;
    get(pool, new.id).await
}

pub async fn get(pool: &AnyPool, id: &str) -> Result<Sticker, AppError> {
    sqlx::query(&super::q(&format!("{SELECT_STICKERS} WHERE id = ?")))
        .bind(id)
        .fetch_optional(pool)
        .await?
        .map(row_to_sticker)
        .ok_or_else(|| AppError::NotFound("unknown sticker".into()))
}

pub async fn list(pool: &AnyPool, space_id: &str) -> Result<Vec<Sticker>, AppError> {
    let rows = sqlx::query(&super::q(&format!(
        "{SELECT_STICKERS} WHERE space_id = ? ORDER BY created_at, id"
    )))
    .bind(space_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(row_to_sticker).collect())
}

pub async fn count(pool: &AnyPool, space_id: &str) -> Result<i64, AppError> {
    Ok(sqlx::query_scalar(&super::q(
        "SELECT COUNT(*) FROM stickers WHERE space_id = ?",
    ))
    .bind(space_id)
    .fetch_one(pool)
    .await?)
}

pub async fn update(
    pool: &AnyPool,
    id: &str,
    name: &str,
    description: Option<&str>,
    tags: Option<&str>,
) -> Result<(), AppError> {
    sqlx::query(&super::q(
        "UPDATE stickers SET name = ?, description = ?, tags = ? WHERE id = ?",
    ))
    .bind(name)
    .bind(description)
    .bind(tags)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn delete(pool: &AnyPool, id: &str) -> Result<(), AppError> {
    sqlx::query(&super::q("DELETE FROM stickers WHERE id = ?"))
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn attach(pool: &AnyPool, message_id: &str, sticker_id: &str) -> Result<(), AppError> {
    sqlx::query(&super::q(
        "INSERT INTO message_stickers (message_id, sticker_id) VALUES (?, ?)",
    ))
    .bind(message_id)
    .bind(sticker_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// The sticker of each message among `ids` that has one, as message JSON
/// carries it. A sticker deleted since still shows its name.
pub async fn for_messages(
    pool: &AnyPool,
    ids: &[String],
) -> Result<HashMap<String, serde_json::Value>, AppError> {
    let mut out = HashMap::new();
    if ids.is_empty() {
        return Ok(out);
    }
    let placeholders = vec!["?"; ids.len()].join(", ");
    let sql = super::q(&format!(
        "SELECT ms.message_id, ms.sticker_id, s.name, s.image, s.format FROM message_stickers ms \
         LEFT JOIN stickers s ON s.id = ms.sticker_id WHERE ms.message_id IN ({placeholders})"
    ));
    let mut query = sqlx::query(&sql);
    for id in ids {
        query = query.bind(id);
    }
    for row in query.fetch_all(pool).await? {
        let message_id: String = row.get("message_id");
        let sticker_id: String = row.get("sticker_id");
        let name: Option<String> = row.get("name");
        let image: Option<String> = row.get("image");
        let format: Option<String> = row.get("format");
        out.insert(
            message_id,
            serde_json::json!([{
                "id": sticker_id,
                "name": name.unwrap_or_else(|| "Deleted sticker".into()),
                "image": image,
                "format": format,
            }]),
        );
    }
    Ok(out)
}
