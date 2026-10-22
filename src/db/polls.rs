use std::collections::HashMap;

use sqlx::{AnyPool, Row};

use crate::error::AppError;
use crate::models::poll::CreatePoll;

/// A poll's settings, without its answers.
#[derive(Debug, Clone)]
pub struct PollRow {
    pub message_id: String,
    pub question: String,
    pub allow_multiselect: bool,
    pub expires_at: String,
}

impl PollRow {
    /// Whether the poll has closed at [now] (formatted like `expires_at`).
    pub fn is_closed(&self, now: &str) -> bool {
        self.expires_at.as_str() <= now
    }
}

/// The current time, formatted like `polls.expires_at`.
pub fn now_string() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S+00:00")
        .to_string()
}

/// Creates the poll for [message_id]; answers are numbered from 1 in order.
pub async fn create_poll(
    pool: &AnyPool,
    message_id: &str,
    poll: &CreatePoll,
    expires_at: &str,
) -> Result<(), AppError> {
    let mut tx = pool.begin().await?;
    sqlx::query(&super::q(
        "INSERT INTO polls (message_id, question, allow_multiselect, expires_at) VALUES (?, ?, ?, ?)",
    ))
    .bind(message_id)
    .bind(poll.question.trim())
    .bind(i64::from(poll.allow_multiselect))
    .bind(expires_at)
    .execute(&mut *tx)
    .await?;
    for (index, answer) in poll.answers.iter().enumerate() {
        sqlx::query(&super::q(
            "INSERT INTO poll_answers (message_id, answer_id, text, emoji) VALUES (?, ?, ?, ?)",
        ))
        .bind(message_id)
        .bind(index as i64 + 1)
        .bind(answer.text.trim())
        .bind(answer.emoji.as_deref())
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// The poll on [message_id], if it has one.
pub async fn get_poll(pool: &AnyPool, message_id: &str) -> Result<Option<PollRow>, AppError> {
    let row = sqlx::query(&super::q(
        "SELECT message_id, question, allow_multiselect, expires_at FROM polls WHERE message_id = ?",
    ))
    .bind(message_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|row| PollRow {
        message_id: row.get("message_id"),
        question: row.get("question"),
        allow_multiselect: crate::db::get_bool(&row, "allow_multiselect"),
        expires_at: row.get("expires_at"),
    }))
}

/// Whether [answer_id] is one of [message_id]'s answers.
pub async fn answer_exists(
    pool: &AnyPool,
    message_id: &str,
    answer_id: i64,
) -> Result<bool, AppError> {
    let found: Option<i64> = sqlx::query_scalar(&super::q(
        "SELECT answer_id FROM poll_answers WHERE message_id = ? AND answer_id = ?",
    ))
    .bind(message_id)
    .bind(answer_id)
    .fetch_optional(pool)
    .await?;
    Ok(found.is_some())
}

/// Records [user_id]'s vote for [answer_id]. A single-choice poll replaces the
/// user's previous vote; voting twice for the same answer is a no-op.
pub async fn vote(
    pool: &AnyPool,
    message_id: &str,
    answer_id: i64,
    user_id: &str,
    multiselect: bool,
) -> Result<(), AppError> {
    let mut tx = pool.begin().await?;
    if !multiselect {
        sqlx::query(&super::q(
            "DELETE FROM poll_votes WHERE message_id = ? AND user_id = ? AND answer_id <> ?",
        ))
        .bind(message_id)
        .bind(user_id)
        .bind(answer_id)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(&super::q(
        "INSERT INTO poll_votes (message_id, answer_id, user_id) VALUES (?, ?, ?) \
         ON CONFLICT (message_id, answer_id, user_id) DO NOTHING",
    ))
    .bind(message_id)
    .bind(answer_id)
    .bind(user_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Removes [user_id]'s vote for [answer_id], if any.
pub async fn unvote(
    pool: &AnyPool,
    message_id: &str,
    answer_id: i64,
    user_id: &str,
) -> Result<(), AppError> {
    sqlx::query(&super::q(
        "DELETE FROM poll_votes WHERE message_id = ? AND answer_id = ? AND user_id = ?",
    ))
    .bind(message_id)
    .bind(answer_id)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Closes the poll now (an early end), unless it already closed.
pub async fn expire(pool: &AnyPool, message_id: &str, now: &str) -> Result<(), AppError> {
    sqlx::query(&super::q(
        "UPDATE polls SET expires_at = ? WHERE message_id = ? AND expires_at > ?",
    ))
    .bind(now)
    .bind(message_id)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// The `poll` JSON for each of [message_ids] that carries a poll: question,
/// answers with their vote counts, the totals and whether it has closed. With
/// [current_user_id], each answer also says whether that user voted for it
/// (`"me"`); broadcasts leave it out.
pub async fn polls_for_messages(
    pool: &AnyPool,
    message_ids: &[String],
    current_user_id: Option<&str>,
) -> Result<HashMap<String, serde_json::Value>, AppError> {
    if message_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let in_clause = message_ids
        .iter()
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(", ");

    let sql = super::q(&format!(
        "SELECT message_id, question, allow_multiselect, expires_at FROM polls WHERE message_id IN ({in_clause})"
    ));
    let mut query = sqlx::query(&sql);
    for id in message_ids {
        query = query.bind(id);
    }
    let polls = query.fetch_all(pool).await?;
    if polls.is_empty() {
        return Ok(HashMap::new());
    }

    let sql = super::q(&format!(
        "SELECT a.message_id, a.answer_id, a.text, a.emoji, \
                (SELECT COUNT(*) FROM poll_votes v WHERE v.message_id = a.message_id AND v.answer_id = a.answer_id) AS votes \
         FROM poll_answers a WHERE a.message_id IN ({in_clause}) ORDER BY a.message_id, a.answer_id"
    ));
    let mut query = sqlx::query(&sql);
    for id in message_ids {
        query = query.bind(id);
    }
    let answers = query.fetch_all(pool).await?;

    let mut mine: std::collections::HashSet<(String, i64)> = std::collections::HashSet::new();
    if let Some(user_id) = current_user_id {
        let sql = super::q(&format!(
            "SELECT message_id, answer_id FROM poll_votes WHERE user_id = ? AND message_id IN ({in_clause})"
        ));
        let mut query = sqlx::query(&sql).bind(user_id);
        for id in message_ids {
            query = query.bind(id);
        }
        for row in query.fetch_all(pool).await? {
            mine.insert((row.get("message_id"), row.get("answer_id")));
        }
    }

    let now = now_string();
    let mut by_message: HashMap<String, Vec<serde_json::Value>> = HashMap::new();
    let mut totals: HashMap<String, i64> = HashMap::new();
    for row in answers {
        let message_id: String = row.get("message_id");
        let answer_id: i64 = row.get("answer_id");
        let votes: i64 = row.get("votes");
        *totals.entry(message_id.clone()).or_default() += votes;
        let mut answer = serde_json::json!({
            "answer_id": answer_id,
            "text": row.get::<String, _>("text"),
            "emoji": row.get::<Option<String>, _>("emoji"),
            "votes": votes,
        });
        if current_user_id.is_some() {
            answer["me"] = serde_json::json!(mine.contains(&(message_id.clone(), answer_id)));
        }
        by_message.entry(message_id).or_default().push(answer);
    }

    let mut result = HashMap::new();
    for row in polls {
        let message_id: String = row.get("message_id");
        let expires_at: String = row.get("expires_at");
        let closed = expires_at.as_str() <= now.as_str();
        result.insert(
            message_id.clone(),
            serde_json::json!({
                "question": row.get::<String, _>("question"),
                "answers": by_message.remove(&message_id).unwrap_or_default(),
                "allow_multiselect": crate::db::get_bool(&row, "allow_multiselect"),
                "expires_at": expires_at,
                "closed": closed,
                "total_votes": totals.get(&message_id).copied().unwrap_or(0),
            }),
        );
    }
    Ok(result)
}
