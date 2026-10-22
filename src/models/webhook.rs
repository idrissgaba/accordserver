use serde::{Deserialize, Serialize};

use super::embed::Embed;

/// A webhook as stored.
#[derive(Debug, Clone, Serialize)]
pub struct Webhook {
    pub id: String,
    pub space_id: String,
    pub channel_id: String,
    /// The bot user its messages are posted as.
    pub user_id: String,
    pub creator_id: Option<String>,
    pub name: String,
    #[serde(skip_serializing)]
    pub token: String,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateWebhook {
    pub name: String,
    /// Avatar as a `data:` URI.
    pub avatar: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateWebhook {
    pub name: Option<String>,
    /// Avatar as a `data:` URI, or `""` to remove it.
    pub avatar: Option<String>,
    /// Move the webhook to another channel of the same space.
    pub channel_id: Option<String>,
}

/// Longest webhook (or per-message) name, as elsewhere for display names.
pub const MAX_WEBHOOK_NAME: usize = 80;

/// Trims and checks a webhook name.
pub fn validate_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("webhook name must not be empty".into());
    }
    if name.chars().count() > MAX_WEBHOOK_NAME {
        return Err(format!(
            "webhook name must be at most {MAX_WEBHOOK_NAME} characters"
        ));
    }
    Ok(name.to_string())
}

/// `allowed_mentions` of an execute call. Only `parse` is honoured.
#[derive(Debug, Default, Deserialize)]
pub struct AllowedMentions {
    #[serde(default)]
    pub parse: Option<Vec<String>>,
}

/// The body of `POST /webhooks/{id}/{token}`, in the format chat services'
/// webhook integrations already speak.
#[derive(Debug, Default, Deserialize)]
pub struct ExecuteWebhook {
    #[serde(default)]
    pub content: Option<String>,
    /// Display name for this message only.
    #[serde(default)]
    pub username: Option<String>,
    /// Accepted for compatibility; remote avatars are not displayed.
    #[serde(default)]
    pub avatar_url: Option<String>,
    #[serde(default)]
    pub tts: Option<bool>,
    /// Kept loose: integrations send embed shapes the strict model rejects
    /// (an empty footer, say); see [`lenient_embeds`].
    #[serde(default)]
    pub embeds: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    pub allowed_mentions: Option<AllowedMentions>,
}

impl ExecuteWebhook {
    /// Whether `@everyone`/`@here` in the content may notify. Off unless the
    /// call asks for it, so a leaked URL cannot ping the whole space.
    pub fn may_mention_everyone(&self) -> bool {
        self.allowed_mentions
            .as_ref()
            .and_then(|a| a.parse.as_ref())
            .is_some_and(|parse| parse.iter().any(|p| p == "everyone"))
    }
}

/// The body of `PATCH /webhooks/{id}/{token}/messages/{message_id}`.
#[derive(Debug, Default, Deserialize)]
pub struct EditWebhookMessage {
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub embeds: Option<Vec<serde_json::Value>>,
}

/// Turns loosely shaped embeds into the strict model: a part that does not
/// fit (a footer without text, an image without a URL) is dropped rather than
/// failing the whole message; an embed left with nothing to show is dropped.
pub fn lenient_embeds(raw: &[serde_json::Value]) -> Vec<Embed> {
    raw.iter()
        .filter_map(|value| {
            let mut object = value.as_object()?.clone();
            for part in ["footer", "image", "thumbnail", "author"] {
                let fits = match object.get(part) {
                    None | Some(serde_json::Value::Null) => true,
                    Some(v) => match part {
                        "footer" => {
                            serde_json::from_value::<super::embed::EmbedFooter>(v.clone()).is_ok()
                        }
                        "image" => {
                            serde_json::from_value::<super::embed::EmbedImage>(v.clone()).is_ok()
                        }
                        "thumbnail" => {
                            serde_json::from_value::<super::embed::EmbedThumbnail>(v.clone())
                                .is_ok()
                        }
                        _ => serde_json::from_value::<super::embed::EmbedAuthor>(v.clone()).is_ok(),
                    },
                };
                if !fits {
                    object.remove(part);
                }
            }
            if let Some(serde_json::Value::Array(fields)) = object.get("fields") {
                let kept: Vec<serde_json::Value> = fields
                    .iter()
                    .filter(|f| {
                        serde_json::from_value::<super::embed::EmbedField>((*f).clone()).is_ok()
                    })
                    .cloned()
                    .collect();
                object.insert("fields".into(), serde_json::Value::Array(kept));
            }
            let embed: Embed = serde_json::from_value(serde_json::Value::Object(object)).ok()?;
            let shows_something = embed.title.is_some()
                || embed.description.is_some()
                || embed.author.is_some()
                || embed.image.is_some()
                || embed.thumbnail.is_some()
                || embed.footer.is_some()
                || embed.fields.as_ref().is_some_and(|f| !f.is_empty());
            shows_something.then_some(embed)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn names_are_trimmed_and_bounded() {
        assert_eq!(validate_name("  Sonarr ").unwrap(), "Sonarr");
        assert!(validate_name("   ").is_err());
        assert!(validate_name(&"x".repeat(81)).is_err());
    }

    #[test]
    fn everyone_pings_need_an_explicit_opt_in() {
        let quiet: ExecuteWebhook =
            serde_json::from_value(json!({"content": "@everyone"})).unwrap();
        assert!(!quiet.may_mention_everyone());
        let loud: ExecuteWebhook = serde_json::from_value(json!({
            "content": "@everyone",
            "allowed_mentions": {"parse": ["users", "everyone"]}
        }))
        .unwrap();
        assert!(loud.may_mention_everyone());
    }

    #[test]
    fn broken_embed_parts_are_dropped_not_fatal() {
        let embeds = lenient_embeds(&[
            json!({
                "title": "Episode grabbed",
                "color": 3066993,
                "footer": {},
                "image": {"width": 3},
                "fields": [{"name": "Quality", "value": "1080p"}, {"name": "broken"}],
                "provider": {"name": "ignored"}
            }),
            json!({"footer": {}}),
            json!("not an object"),
        ]);
        assert_eq!(embeds.len(), 1);
        let embed = &embeds[0];
        assert_eq!(embed.title.as_deref(), Some("Episode grabbed"));
        assert!(embed.footer.is_none() && embed.image.is_none());
        assert_eq!(embed.fields.as_ref().unwrap().len(), 1);
    }
}
