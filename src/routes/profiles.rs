//! Profile extras — pronouns and a two-colour profile theme — and the private
//! notes a user keeps about others.

use axum::extract::{Path, State};
use axum::Json;
use serde_json::Value;

use crate::db;
use crate::db::profiles::ProfileExtras;
use crate::error::AppError;
use crate::gateway::events::GatewayBroadcast;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

/// Longest pronouns and note.
const MAX_PRONOUNS: usize = 40;
const MAX_NOTE: usize = 256;

fn profile_json(user_id: &str, extras: &ProfileExtras, note: Option<String>) -> Value {
    let mut json = serde_json::json!({
        "user_id": user_id,
        "pronouns": extras.pronouns,
        "theme_primary": extras.theme_primary,
        "theme_accent": extras.theme_accent,
    });
    if let Some(note) = note {
        json["note"] = Value::String(note);
    }
    json
}

/// Applies the profile fields present in `body` to `extras`: `pronouns`
/// (text, `""`/null clears) and `theme_primary` / `theme_accent` (0xRRGGBB,
/// null clears).
pub fn apply_profile(extras: &mut ProfileExtras, body: &Value) -> Result<(), String> {
    let body = body.as_object().ok_or("expected an object")?;
    if let Some(value) = body.get("pronouns") {
        extras.pronouns = match value {
            Value::Null => None,
            Value::String(text) => {
                let text = text.trim();
                if text.chars().count() > MAX_PRONOUNS {
                    return Err(format!("pronouns have at most {MAX_PRONOUNS} characters"));
                }
                (!text.is_empty()).then(|| text.to_string())
            }
            _ => return Err("pronouns must be text".into()),
        };
    }
    for (key, slot) in [
        ("theme_primary", &mut extras.theme_primary),
        ("theme_accent", &mut extras.theme_accent),
    ] {
        if let Some(value) = body.get(key) {
            *slot = match value {
                Value::Null => None,
                other => match other.as_i64() {
                    Some(color) if (0..=0xFF_FFFF).contains(&color) => Some(color),
                    _ => return Err(format!("{key} must be a colour (0xRRGGBB)")),
                },
            };
        }
    }
    Ok(())
}

/// GET /users/{user_id}/profile — with the caller's note about them.
pub async fn get_profile(
    state: State<AppState>,
    Path(user_id): Path<String>,
    auth: AuthUser,
) -> Result<Json<Value>, AppError> {
    let user_id = if user_id == "@me" {
        auth.user_id.clone()
    } else {
        user_id
    };
    db::users::get_user(&state.db, &user_id).await?;
    let extras = db::profiles::get(&state.db, &user_id).await?;
    let note = if user_id == auth.user_id {
        None
    } else {
        db::profiles::note(&state.db, &auth.user_id, &user_id).await?
    };
    Ok(Json(
        serde_json::json!({ "data": profile_json(&user_id, &extras, note) }),
    ))
}

/// GET /users/@me/profile
pub async fn get_my_profile(
    state: State<AppState>,
    auth: AuthUser,
) -> Result<Json<Value>, AppError> {
    let extras = db::profiles::get(&state.db, &auth.user_id).await?;
    Ok(Json(
        serde_json::json!({ "data": profile_json(&auth.user_id, &extras, None) }),
    ))
}

/// PATCH /users/@me/profile
pub async fn update_profile(
    state: State<AppState>,
    auth: AuthUser,
    Json(body): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let mut extras = db::profiles::get(&state.db, &auth.user_id).await?;
    apply_profile(&mut extras, &body).map_err(AppError::BadRequest)?;
    db::profiles::set(&state.db, &auth.user_id, &extras).await?;
    let json = profile_json(&auth.user_id, &extras, None);
    // Shown in profile cards wherever people can see us.
    let spaces = db::users::get_user_spaces(&state.db, &auth.user_id)
        .await
        .unwrap_or_default();
    if let Some(ref tx) = *state.gateway_tx.read().await {
        for space_id in spaces {
            let _ = tx.send(GatewayBroadcast {
                space_id: Some(space_id),
                target_user_ids: None,
                event: serde_json::json!({ "op": 0, "type": "user.profile_update", "data": json }),
                intent: "spaces".to_string(),
                required_permission: None,
            });
        }
        let _ = tx.send(GatewayBroadcast {
            space_id: None,
            target_user_ids: Some(vec![auth.user_id.clone()]),
            event: serde_json::json!({ "op": 0, "type": "user.profile_update", "data": json }),
            intent: "spaces".to_string(),
            required_permission: None,
        });
    }
    Ok(Json(serde_json::json!({ "data": json })))
}

/// GET /users/@me/notes — every note the caller keeps.
pub async fn list_notes(state: State<AppState>, auth: AuthUser) -> Result<Json<Value>, AppError> {
    let notes: serde_json::Map<String, Value> = db::profiles::notes_of(&state.db, &auth.user_id)
        .await?
        .into_iter()
        .map(|(target, note)| (target, Value::String(note)))
        .collect();
    Ok(Json(serde_json::json!({ "data": notes })))
}

/// PUT /users/@me/notes/{user_id} `{note}` — only the caller ever sees it;
/// an empty note removes it.
pub async fn set_note(
    state: State<AppState>,
    Path(target_id): Path<String>,
    auth: AuthUser,
    Json(body): Json<Value>,
) -> Result<Json<Value>, AppError> {
    if target_id == auth.user_id {
        return Err(AppError::BadRequest(
            "you can't keep a note about yourself".into(),
        ));
    }
    db::users::get_user(&state.db, &target_id).await?;
    let note = body
        .get("note")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if note.chars().count() > MAX_NOTE {
        return Err(AppError::BadRequest(format!(
            "a note has at most {MAX_NOTE} characters"
        )));
    }
    db::profiles::set_note(&state.db, &auth.user_id, &target_id, &note).await?;
    if let Some(ref tx) = *state.gateway_tx.read().await {
        let _ = tx.send(GatewayBroadcast {
            space_id: None,
            target_user_ids: Some(vec![auth.user_id.clone()]),
            event: serde_json::json!({
                "op": 0,
                "type": "user.note_update",
                "data": { "user_id": target_id, "note": note },
            }),
            intent: "spaces".to_string(),
            required_permission: None,
        });
    }
    Ok(Json(
        serde_json::json!({ "data": { "user_id": target_id, "note": note } }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn profile_fields_are_checked_and_cleared() {
        let mut extras = ProfileExtras::default();
        apply_profile(
            &mut extras,
            &json!({"pronouns": " she/her ", "theme_primary": 0x5865F2, "theme_accent": 16777215}),
        )
        .unwrap();
        assert_eq!(extras.pronouns.as_deref(), Some("she/her"));
        assert_eq!(extras.theme_primary, Some(0x5865F2));
        // Absent fields stay; null or empty clears.
        apply_profile(&mut extras, &json!({"pronouns": "", "theme_accent": null})).unwrap();
        assert_eq!(extras.pronouns, None);
        assert_eq!(extras.theme_primary, Some(0x5865F2));
        assert_eq!(extras.theme_accent, None);
        assert!(apply_profile(&mut extras, &json!({"theme_primary": 0x1000000})).is_err());
        assert!(apply_profile(&mut extras, &json!({"pronouns": "x".repeat(41)})).is_err());
        assert!(apply_profile(&mut extras, &json!({"pronouns": 3})).is_err());
    }
}
