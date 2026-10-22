use std::collections::HashMap;

use sqlx::{AnyPool, Row};

use crate::error::AppError;
use crate::models::webhook::Webhook;
use crate::snowflake;

const SELECT_WEBHOOKS: &str =
    "SELECT id, space_id, channel_id, user_id, creator_id, name, token, created_at FROM webhooks";

fn row_to_webhook(row: sqlx::any::AnyRow) -> Webhook {
    Webhook {
        id: row.get("id"),
        space_id: row.get("space_id"),
        channel_id: row.get("channel_id"),
        user_id: row.get("user_id"),
        creator_id: row.get("creator_id"),
        name: row.get("name"),
        token: row.get("token"),
        created_at: row.get("created_at"),
    }
}

/// A fresh secret for a webhook URL.
fn generate_token() -> String {
    use rand::Rng;
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut rng = rand::thread_rng();
    (0..68)
        .map(|_| CHARSET[rng.gen_range(0..CHARSET.len())] as char)
        .collect()
}

/// Compares a presented token with the stored one in constant time.
pub fn token_matches(stored: &str, presented: &str) -> bool {
    stored.len() == presented.len()
        && stored
            .bytes()
            .zip(presented.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

/// Creates a webhook in `channel_id` together with the bot user it posts as.
pub async fn create(
    pool: &AnyPool,
    space_id: &str,
    channel_id: &str,
    creator_id: &str,
    name: &str,
) -> Result<Webhook, AppError> {
    let id = snowflake::generate();
    let user_id = snowflake::generate();
    let mut tx = pool.begin().await?;
    sqlx::query(&super::q(
        "INSERT INTO users (id, username, display_name, bot) VALUES (?, ?, ?, TRUE)",
    ))
    .bind(&user_id)
    .bind(format!("webhook-{id}"))
    .bind(name)
    .execute(&mut *tx)
    .await?;
    sqlx::query(&super::q(
        "INSERT INTO webhooks (id, space_id, channel_id, user_id, creator_id, name, token) VALUES (?, ?, ?, ?, ?, ?, ?)",
    ))
    .bind(&id)
    .bind(space_id)
    .bind(channel_id)
    .bind(&user_id)
    .bind(creator_id)
    .bind(name)
    .bind(generate_token())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    get(pool, &id).await
}

pub async fn get(pool: &AnyPool, id: &str) -> Result<Webhook, AppError> {
    sqlx::query(&super::q(&format!("{SELECT_WEBHOOKS} WHERE id = ?")))
        .bind(id)
        .fetch_optional(pool)
        .await?
        .map(row_to_webhook)
        .ok_or_else(|| AppError::NotFound("unknown webhook".into()))
}

pub async fn list_for_channel(pool: &AnyPool, channel_id: &str) -> Result<Vec<Webhook>, AppError> {
    let rows = sqlx::query(&super::q(&format!(
        "{SELECT_WEBHOOKS} WHERE channel_id = ? ORDER BY created_at, id"
    )))
    .bind(channel_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(row_to_webhook).collect())
}

pub async fn list_for_space(pool: &AnyPool, space_id: &str) -> Result<Vec<Webhook>, AppError> {
    let rows = sqlx::query(&super::q(&format!(
        "{SELECT_WEBHOOKS} WHERE space_id = ? ORDER BY created_at, id"
    )))
    .bind(space_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(row_to_webhook).collect())
}

/// Renames a webhook and the bot user it posts as.
pub async fn rename(pool: &AnyPool, webhook: &Webhook, name: &str) -> Result<(), AppError> {
    let mut tx = pool.begin().await?;
    sqlx::query(&super::q("UPDATE webhooks SET name = ? WHERE id = ?"))
        .bind(name)
        .bind(&webhook.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(&super::q("UPDATE users SET display_name = ? WHERE id = ?"))
        .bind(name)
        .bind(&webhook.user_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// Points the webhook's bot user at a stored avatar (or none).
pub async fn set_avatar(
    pool: &AnyPool,
    webhook: &Webhook,
    avatar: Option<&str>,
) -> Result<(), AppError> {
    sqlx::query(&super::q("UPDATE users SET avatar = ? WHERE id = ?"))
        .bind(avatar)
        .bind(&webhook.user_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn move_to_channel(pool: &AnyPool, id: &str, channel_id: &str) -> Result<(), AppError> {
    sqlx::query(&super::q("UPDATE webhooks SET channel_id = ? WHERE id = ?"))
        .bind(channel_id)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Deletes the webhook. Its bot user stays, so its past messages keep their
/// author.
pub async fn delete(pool: &AnyPool, id: &str) -> Result<(), AppError> {
    sqlx::query(&super::q("DELETE FROM webhooks WHERE id = ?"))
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Marks `message_id` as posted by `webhook_id` (with the per-message display
/// name, if any) and settles whether it may notify everyone.
pub async fn record_message(
    pool: &AnyPool,
    message_id: &str,
    webhook_id: &str,
    username: Option<&str>,
    mention_everyone: bool,
) -> Result<(), AppError> {
    let mut tx = pool.begin().await?;
    let flag: &str = if mention_everyone {
        "UPDATE messages SET webhook_id = ? WHERE id = ?"
    } else {
        "UPDATE messages SET webhook_id = ?, mention_everyone = FALSE WHERE id = ?"
    };
    sqlx::query(&super::q(flag))
        .bind(webhook_id)
        .bind(message_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(&super::q(
        "INSERT INTO webhook_messages (message_id, webhook_id, username) VALUES (?, ?, ?)",
    ))
    .bind(message_id)
    .bind(webhook_id)
    .bind(username)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// The per-message display names of the webhook messages among `ids`.
pub async fn usernames_for_messages(
    pool: &AnyPool,
    ids: &[String],
) -> Result<HashMap<String, String>, AppError> {
    let mut names = HashMap::new();
    if ids.is_empty() {
        return Ok(names);
    }
    let placeholders = vec!["?"; ids.len()].join(", ");
    let sql = format!(
        "SELECT message_id, username FROM webhook_messages WHERE username IS NOT NULL AND message_id IN ({placeholders})"
    );
    let query = super::q(&sql);
    let mut q = sqlx::query(&query);
    for id in ids {
        q = q.bind(id);
    }
    for row in q.fetch_all(pool).await? {
        let message_id: String = row.get("message_id");
        let username: String = row.get("username");
        names.insert(message_id, username);
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_long_and_url_safe() {
        let token = generate_token();
        assert_eq!(token.len(), 68);
        assert!(token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        assert_ne!(token, generate_token());
    }

    #[test]
    fn token_comparison_needs_an_exact_match() {
        assert!(token_matches("abc", "abc"));
        assert!(!token_matches("abc", "abd"));
        assert!(!token_matches("abc", "abcd"));
        assert!(!token_matches("abc", ""));
    }
}
