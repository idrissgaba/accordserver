use std::collections::HashMap;

use sqlx::{AnyPool, Row};

use crate::error::AppError;

/// Adds `user_id`'s super reaction; false when it was already there.
pub async fn add(
    pool: &AnyPool,
    is_postgres: bool,
    message_id: &str,
    user_id: &str,
    emoji: &str,
) -> Result<bool, AppError> {
    let sql = if is_postgres {
        "INSERT INTO super_reactions (message_id, user_id, emoji_name) VALUES (?, ?, ?) \
         ON CONFLICT DO NOTHING"
    } else {
        "INSERT OR IGNORE INTO super_reactions (message_id, user_id, emoji_name) VALUES (?, ?, ?)"
    };
    let result = sqlx::query(&super::q(sql))
        .bind(message_id)
        .bind(user_id)
        .bind(emoji)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Removes `user_id`'s super reaction; false when there was none.
pub async fn remove(
    pool: &AnyPool,
    message_id: &str,
    user_id: &str,
    emoji: &str,
) -> Result<bool, AppError> {
    let result = sqlx::query(&super::q(
        "DELETE FROM super_reactions WHERE message_id = ? AND user_id = ? AND emoji_name = ?",
    ))
    .bind(message_id)
    .bind(user_id)
    .bind(emoji)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Each message's super reactions as `[{emoji, count, me}]`, first added first.
pub async fn for_messages(
    pool: &AnyPool,
    ids: &[String],
    current_user_id: Option<&str>,
) -> Result<HashMap<String, serde_json::Value>, AppError> {
    let mut out: HashMap<String, Vec<serde_json::Value>> = HashMap::new();
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = vec!["?"; ids.len()].join(", ");
    let sql = super::q(&format!(
        "SELECT message_id, emoji_name, COUNT(*) AS cnt, \
         SUM(CASE WHEN user_id = ? THEN 1 ELSE 0 END) AS mine \
         FROM super_reactions WHERE message_id IN ({placeholders}) \
         GROUP BY message_id, emoji_name ORDER BY message_id, MIN(created_at)"
    ));
    let mut query = sqlx::query(&sql).bind(current_user_id.unwrap_or(""));
    for id in ids {
        query = query.bind(id);
    }
    for row in query.fetch_all(pool).await? {
        let message_id: String = row.get("message_id");
        let emoji: String = row.get("emoji_name");
        let count: i64 = row.get("cnt");
        let mine: i64 = row.get("mine");
        out.entry(message_id).or_default().push(serde_json::json!({
            "emoji": emoji,
            "count": count,
            "me": mine > 0,
        }));
    }
    Ok(out
        .into_iter()
        .map(|(id, list)| (id, serde_json::Value::Array(list)))
        .collect())
}
