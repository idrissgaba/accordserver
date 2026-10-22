use sqlx::{AnyPool, Row};

use crate::error::AppError;
use crate::models::app_command::{CommandDefinition, CommandOptionDef};
use crate::snowflake;

/// The parts of an application the command and interaction APIs need.
#[derive(Debug, Clone)]
pub struct AppInfo {
    pub id: String,
    pub name: String,
    pub owner_id: String,
    pub bot_user_id: Option<String>,
}

/// A registered slash command.
#[derive(Debug, Clone)]
pub struct CommandRow {
    pub id: String,
    pub application_id: String,
    pub space_id: Option<String>,
    pub name: String,
    pub description: String,
    pub options: Vec<CommandOptionDef>,
}

impl CommandRow {
    pub fn definition(&self) -> CommandDefinition {
        CommandDefinition {
            name: self.name.clone(),
            description: self.description.clone(),
            options: self.options.clone(),
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "application_id": self.application_id,
            "space_id": self.space_id,
            "type": "chat_input",
            "name": self.name,
            "description": self.description,
            "options": self.options,
        })
    }
}

fn row_to_app(row: sqlx::any::AnyRow) -> AppInfo {
    AppInfo {
        id: row.get("id"),
        name: row.get("name"),
        owner_id: row.get("owner_id"),
        bot_user_id: row.get("bot_user_id"),
    }
}

fn row_to_command(row: sqlx::any::AnyRow) -> CommandRow {
    let options: String = row.get("options");
    CommandRow {
        id: row.get("id"),
        application_id: row.get("application_id"),
        space_id: row.get("space_id"),
        name: row.get("name"),
        description: row.get("description"),
        options: serde_json::from_str(&options).unwrap_or_default(),
    }
}

const SELECT_COMMANDS: &str =
    "SELECT id, application_id, space_id, name, description, options FROM application_commands";

pub async fn get_application(pool: &AnyPool, app_id: &str) -> Result<AppInfo, AppError> {
    sqlx::query(&super::q(
        "SELECT id, name, owner_id, bot_user_id FROM applications WHERE id = ?",
    ))
    .bind(app_id)
    .fetch_optional(pool)
    .await?
    .map(row_to_app)
    .ok_or_else(|| AppError::NotFound("unknown application".into()))
}

/// The application whose bot is `user_id`, if any.
pub async fn application_by_bot_user(
    pool: &AnyPool,
    user_id: &str,
) -> Result<Option<AppInfo>, AppError> {
    Ok(sqlx::query(&super::q(
        "SELECT id, name, owner_id, bot_user_id FROM applications WHERE bot_user_id = ?",
    ))
    .bind(user_id)
    .fetch_optional(pool)
    .await?
    .map(row_to_app))
}

/// The application `app_id`, for its bot or its owner.
pub async fn application_for_caller(
    pool: &AnyPool,
    app_id: &str,
    user_id: &str,
) -> Result<AppInfo, AppError> {
    let app = get_application(pool, app_id).await?;
    if app.owner_id != user_id && app.bot_user_id.as_deref() != Some(user_id) {
        return Err(AppError::Forbidden(
            "only the application's bot or owner can manage its commands".into(),
        ));
    }
    Ok(app)
}

pub async fn list_commands(
    pool: &AnyPool,
    app_id: &str,
    space_id: Option<&str>,
) -> Result<Vec<CommandRow>, AppError> {
    let rows = match space_id {
        Some(space_id) => {
            sqlx::query(&super::q(&format!(
                "{SELECT_COMMANDS} WHERE application_id = ? AND space_id = ? ORDER BY name"
            )))
            .bind(app_id)
            .bind(space_id)
            .fetch_all(pool)
            .await?
        }
        None => {
            sqlx::query(&super::q(&format!(
                "{SELECT_COMMANDS} WHERE application_id = ? AND space_id IS NULL ORDER BY name"
            )))
            .bind(app_id)
            .fetch_all(pool)
            .await?
        }
    };
    Ok(rows.into_iter().map(row_to_command).collect())
}

pub async fn get_command(pool: &AnyPool, command_id: &str) -> Result<CommandRow, AppError> {
    sqlx::query(&super::q(&format!("{SELECT_COMMANDS} WHERE id = ?")))
        .bind(command_id)
        .fetch_optional(pool)
        .await?
        .map(row_to_command)
        .ok_or_else(|| AppError::NotFound("unknown command".into()))
}

async fn upsert_in(
    conn: &mut sqlx::AnyConnection,
    app_id: &str,
    space_id: Option<&str>,
    def: &CommandDefinition,
) -> Result<(), AppError> {
    let options = serde_json::to_string(&def.options).unwrap_or_else(|_| "[]".into());
    let existing: Option<String> = match space_id {
        Some(space_id) => sqlx::query_scalar(&super::q(
            "SELECT id FROM application_commands WHERE application_id = ? AND space_id = ? AND name = ?",
        ))
        .bind(app_id)
        .bind(space_id)
        .bind(&def.name)
        .fetch_optional(&mut *conn)
        .await?,
        None => sqlx::query_scalar(&super::q(
            "SELECT id FROM application_commands WHERE application_id = ? AND space_id IS NULL AND name = ?",
        ))
        .bind(app_id)
        .bind(&def.name)
        .fetch_optional(&mut *conn)
        .await?,
    };
    match existing {
        Some(id) => {
            sqlx::query(&super::q(
                "UPDATE application_commands SET description = ?, options = ? WHERE id = ?",
            ))
            .bind(&def.description)
            .bind(&options)
            .bind(&id)
            .execute(&mut *conn)
            .await?;
        }
        None => {
            sqlx::query(&super::q(
                "INSERT INTO application_commands (id, application_id, space_id, name, description, options) VALUES (?, ?, ?, ?, ?, ?)",
            ))
            .bind(snowflake::generate())
            .bind(app_id)
            .bind(space_id)
            .bind(&def.name)
            .bind(&def.description)
            .bind(&options)
            .execute(&mut *conn)
            .await?;
        }
    }
    Ok(())
}

/// Creates the command, or updates the one with the same name.
pub async fn upsert_command(
    pool: &AnyPool,
    app_id: &str,
    space_id: Option<&str>,
    def: &CommandDefinition,
) -> Result<CommandRow, AppError> {
    let mut conn = pool.acquire().await?;
    upsert_in(&mut conn, app_id, space_id, def).await?;
    drop(conn);
    list_commands(pool, app_id, space_id)
        .await?
        .into_iter()
        .find(|c| c.name == def.name)
        .ok_or_else(|| AppError::Internal("command vanished after saving".into()))
}

/// Makes the application's commands (global, or of one space) exactly `defs`.
pub async fn replace_commands(
    pool: &AnyPool,
    app_id: &str,
    space_id: Option<&str>,
    defs: &[CommandDefinition],
) -> Result<Vec<CommandRow>, AppError> {
    let mut tx = pool.begin().await?;
    let existing = match space_id {
        Some(space_id) => sqlx::query(&super::q(
            "SELECT id, name FROM application_commands WHERE application_id = ? AND space_id = ?",
        ))
        .bind(app_id)
        .bind(space_id)
        .fetch_all(&mut *tx)
        .await?,
        None => sqlx::query(&super::q(
            "SELECT id, name FROM application_commands WHERE application_id = ? AND space_id IS NULL",
        ))
        .bind(app_id)
        .fetch_all(&mut *tx)
        .await?,
    };
    for row in existing {
        let name: String = row.get("name");
        if !defs.iter().any(|d| d.name == name) {
            let id: String = row.get("id");
            sqlx::query(&super::q("DELETE FROM application_commands WHERE id = ?"))
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
    }
    for def in defs {
        upsert_in(&mut tx, app_id, space_id, def).await?;
    }
    tx.commit().await?;
    list_commands(pool, app_id, space_id).await
}

pub async fn delete_command(
    pool: &AnyPool,
    app_id: &str,
    command_id: &str,
) -> Result<(), AppError> {
    let deleted = sqlx::query(&super::q(
        "DELETE FROM application_commands WHERE id = ? AND application_id = ?",
    ))
    .bind(command_id)
    .bind(app_id)
    .execute(pool)
    .await?;
    if deleted.rows_affected() == 0 {
        return Err(AppError::NotFound("unknown command".into()));
    }
    Ok(())
}

/// The commands members of `space_id` can use: those of every application
/// whose bot is a member there, global or registered for that space.
pub async fn commands_for_space(
    pool: &AnyPool,
    space_id: &str,
) -> Result<Vec<(CommandRow, AppInfo)>, AppError> {
    let apps: Vec<AppInfo> = sqlx::query(&super::q(
        "SELECT a.id, a.name, a.owner_id, a.bot_user_id FROM applications a \
         JOIN members m ON m.user_id = a.bot_user_id AND m.space_id = ?",
    ))
    .bind(space_id)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(row_to_app)
    .collect();
    let mut out = Vec::new();
    for app in apps {
        let rows = sqlx::query(&super::q(&format!(
            "{SELECT_COMMANDS} WHERE application_id = ? AND (space_id IS NULL OR space_id = ?) ORDER BY name"
        )))
        .bind(&app.id)
        .bind(space_id)
        .fetch_all(pool)
        .await?;
        for row in rows {
            out.push((row_to_command(row), app.clone()));
        }
    }
    Ok(out)
}

/// The spaces a user (an application's bot) is a member of.
pub async fn spaces_of_user(pool: &AnyPool, user_id: &str) -> Result<Vec<String>, AppError> {
    Ok(
        sqlx::query_scalar(&super::q("SELECT space_id FROM members WHERE user_id = ?"))
            .bind(user_id)
            .fetch_all(pool)
            .await?,
    )
}
