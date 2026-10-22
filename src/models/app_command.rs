//! Slash commands, their invocation options, and message components (buttons
//! and select menus): the shapes applications send and how they are checked.
//!
//! Option and component types are words (`"string"`, `"button"`…); the
//! numeric codes chat-bot libraries use are accepted too.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Option types, by word and by the numeric code bot libraries send.
const OPTION_TYPES: &[(&str, i64)] = &[
    ("string", 3),
    ("integer", 4),
    ("boolean", 5),
    ("user", 6),
    ("channel", 7),
    ("role", 8),
    ("number", 10),
];

fn option_type_word(raw: &Value) -> Option<&'static str> {
    match raw {
        Value::String(word) => OPTION_TYPES
            .iter()
            .find(|(w, _)| w == word)
            .map(|(w, _)| *w),
        Value::Number(code) => OPTION_TYPES
            .iter()
            .find(|(_, c)| Some(*c) == code.as_i64())
            .map(|(w, _)| *w),
        _ => None,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommandChoice {
    pub name: String,
    pub value: Value,
}

/// One option of a slash command, as registered.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommandOptionDef {
    pub name: String,
    pub description: String,
    /// Normalised to a word on validation.
    #[serde(rename = "type")]
    pub option_type: Value,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choices: Option<Vec<CommandChoice>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_value: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_value: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_length: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_length: Option<u32>,
}

/// A slash command as an application registers it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommandDefinition {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub options: Vec<CommandOptionDef>,
}

pub const MAX_COMMANDS_PER_APPLICATION: usize = 100;
const MAX_OPTIONS: usize = 25;
const MAX_CHOICES: usize = 25;

fn valid_command_name(name: &str) -> bool {
    (1..=32).contains(&name.chars().count())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

fn valid_description(text: &str) -> bool {
    (1..=100).contains(&text.trim().chars().count())
}

impl CommandDefinition {
    /// Checks the definition and normalises option types to words.
    pub fn validate(mut self) -> Result<Self, String> {
        if !valid_command_name(&self.name) {
            return Err(format!(
                "command name '{}' must be 1-32 lowercase letters, digits, - or _",
                self.name
            ));
        }
        if !valid_description(&self.description) {
            return Err(format!(
                "command '{}' needs a description of 1-100 characters",
                self.name
            ));
        }
        if self.options.len() > MAX_OPTIONS {
            return Err(format!("a command has at most {MAX_OPTIONS} options"));
        }
        let mut seen_optional = false;
        let mut names = std::collections::HashSet::new();
        for option in &mut self.options {
            if !valid_command_name(&option.name) {
                return Err(format!(
                    "option name '{}' must be 1-32 lowercase letters, digits, - or _",
                    option.name
                ));
            }
            if !names.insert(option.name.clone()) {
                return Err(format!("option '{}' appears twice", option.name));
            }
            if !valid_description(&option.description) {
                return Err(format!(
                    "option '{}' needs a description of 1-100 characters",
                    option.name
                ));
            }
            let word = option_type_word(&option.option_type)
                .ok_or_else(|| format!("option '{}' has an unknown type", option.name))?;
            option.option_type = Value::String(word.into());
            if option.required && seen_optional {
                return Err("required options must come before optional ones".into());
            }
            seen_optional |= !option.required;
            if let Some(ref choices) = option.choices {
                if choices.is_empty() || choices.len() > MAX_CHOICES {
                    return Err(format!(
                        "option '{}' must have 1-{MAX_CHOICES} choices",
                        option.name
                    ));
                }
                if !matches!(word, "string" | "integer" | "number") {
                    return Err(format!(
                        "option '{}': only text and number options have choices",
                        option.name
                    ));
                }
                for choice in choices {
                    if !(1..=100).contains(&choice.name.chars().count())
                        || value_fits(word, &choice.value).is_err()
                    {
                        return Err(format!("option '{}' has an invalid choice", option.name));
                    }
                }
            }
        }
        Ok(self)
    }
}

/// Whether `value` is of the option type `word` (ids for users, channels and
/// roles are strings; existence is checked by the caller).
fn value_fits(word: &str, value: &Value) -> Result<(), ()> {
    let fits = match word {
        "string" | "user" | "channel" | "role" => value.is_string(),
        "integer" => value.as_i64().is_some(),
        "number" => value.as_f64().is_some(),
        "boolean" => value.is_boolean(),
        _ => false,
    };
    if fits {
        Ok(())
    } else {
        Err(())
    }
}

/// One option value a member sent with a command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptionValue {
    pub name: String,
    pub value: Value,
}

/// An option value checked against the command, with its type attached for
/// the application.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ResolvedOption {
    pub name: String,
    #[serde(rename = "type")]
    pub option_type: String,
    pub value: Value,
}

/// Checks a command invocation's options against the definition: no unknown
/// or repeated option, every required one present, values of the right type,
/// within bounds and among the choices. Ids (users, channels, roles) are
/// returned for the caller to check against the space.
pub fn resolve_options(
    definition: &CommandDefinition,
    given: &[OptionValue],
) -> Result<Vec<ResolvedOption>, String> {
    let mut resolved = Vec::new();
    for value in given {
        let def = definition
            .options
            .iter()
            .find(|o| o.name == value.name)
            .ok_or_else(|| format!("unknown option '{}'", value.name))?;
        if resolved
            .iter()
            .any(|r: &ResolvedOption| r.name == value.name)
        {
            return Err(format!("option '{}' given twice", value.name));
        }
        let word = option_type_word(&def.option_type).unwrap_or("string");
        // Members' clients may send numbers typed into a text field as text.
        let value_json = match (word, &value.value) {
            ("integer", Value::String(text)) => text
                .trim()
                .parse::<i64>()
                .map(Value::from)
                .map_err(|_| format!("option '{}' must be a whole number", value.name))?,
            ("number", Value::String(text)) => text
                .trim()
                .parse::<f64>()
                .ok()
                .and_then(serde_json::Number::from_f64)
                .map(Value::Number)
                .ok_or_else(|| format!("option '{}' must be a number", value.name))?,
            _ => value.value.clone(),
        };
        value_fits(word, &value_json)
            .map_err(|_| format!("option '{}' must be a {word}", value.name))?;
        if let Some(text) = value_json.as_str() {
            let len = text.chars().count() as u32;
            if word == "string" && text.trim().is_empty() {
                return Err(format!("option '{}' must not be empty", value.name));
            }
            if def.min_length.is_some_and(|min| len < min)
                || def.max_length.is_some_and(|max| len > max)
                || len > 6000
            {
                return Err(format!("option '{}' has the wrong length", value.name));
            }
        }
        if let Some(number) = value_json.as_f64() {
            if def.min_value.is_some_and(|min| number < min)
                || def.max_value.is_some_and(|max| number > max)
            {
                return Err(format!("option '{}' is out of range", value.name));
            }
        }
        if let Some(ref choices) = def.choices {
            if !choices.iter().any(|c| c.value == value_json) {
                return Err(format!(
                    "option '{}' must be one of its choices",
                    value.name
                ));
            }
        }
        resolved.push(ResolvedOption {
            name: value.name.clone(),
            option_type: word.to_string(),
            value: value_json,
        });
    }
    for def in &definition.options {
        if def.required && !resolved.iter().any(|r| r.name == def.name) {
            return Err(format!("option '{}' is required", def.name));
        }
    }
    Ok(resolved)
}

/// Button styles, by word and by numeric code.
const BUTTON_STYLES: &[(&str, i64)] = &[
    ("primary", 1),
    ("secondary", 2),
    ("success", 3),
    ("danger", 4),
    ("link", 5),
];

fn component_type_word(raw: Option<&Value>) -> Option<&'static str> {
    match raw? {
        Value::String(word) => match word.as_str() {
            "action_row" => Some("action_row"),
            "button" => Some("button"),
            "select" | "string_select" => Some("select"),
            _ => None,
        },
        Value::Number(code) => match code.as_i64()? {
            1 => Some("action_row"),
            2 => Some("button"),
            3 => Some("select"),
            _ => None,
        },
        _ => None,
    }
}

fn button_style_word(raw: Option<&Value>) -> Option<&'static str> {
    match raw {
        None => Some("secondary"),
        Some(Value::String(word)) => BUTTON_STYLES
            .iter()
            .find(|(w, _)| w == word)
            .map(|(w, _)| *w),
        Some(Value::Number(code)) => BUTTON_STYLES
            .iter()
            .find(|(_, c)| Some(*c) == code.as_i64())
            .map(|(w, _)| *w),
        _ => None,
    }
}

fn text_field<'a>(object: &'a serde_json::Map<String, Value>, key: &str) -> Option<&'a str> {
    object.get(key).and_then(Value::as_str)
}

fn check_custom_id(object: &serde_json::Map<String, Value>) -> Result<String, String> {
    let id = text_field(object, "custom_id").unwrap_or_default();
    if !(1..=100).contains(&id.chars().count()) {
        return Err("a component needs a custom_id of 1-100 characters".into());
    }
    Ok(id.to_string())
}

/// Checks and normalises a message's components: up to 5 rows, each with up
/// to 5 buttons or a single select menu; custom ids unique in the message.
/// Returns them with type and style words.
pub fn validate_components(raw: &Value) -> Result<Value, String> {
    let rows = raw.as_array().ok_or("components must be a list of rows")?;
    if rows.len() > 5 {
        return Err("a message has at most 5 component rows".into());
    }
    let mut custom_ids = std::collections::HashSet::new();
    let mut out_rows = Vec::new();
    for row in rows {
        let row = row.as_object().ok_or("a component row must be an object")?;
        if component_type_word(row.get("type")) != Some("action_row") {
            return Err("top-level components must be action rows".into());
        }
        let items = row
            .get("components")
            .and_then(Value::as_array)
            .ok_or("an action row needs components")?;
        if items.is_empty() || items.len() > 5 {
            return Err("an action row holds 1-5 components".into());
        }
        let mut out_items = Vec::new();
        for item in items {
            let item = item.as_object().ok_or("a component must be an object")?;
            match component_type_word(item.get("type")) {
                Some("button") => {
                    let style =
                        button_style_word(item.get("style")).ok_or("unknown button style")?;
                    let label = text_field(item, "label").unwrap_or_default();
                    let emoji = item.get("emoji").cloned().unwrap_or(Value::Null);
                    if label.chars().count() > 80 || (label.is_empty() && emoji.is_null()) {
                        return Err(
                            "a button needs a label (up to 80 characters) or an emoji".into()
                        );
                    }
                    let mut button = serde_json::json!({
                        "type": "button",
                        "style": style,
                        "label": label,
                        "disabled": item.get("disabled").and_then(Value::as_bool).unwrap_or(false),
                    });
                    if !emoji.is_null() {
                        button["emoji"] = emoji;
                    }
                    if style == "link" {
                        let url = text_field(item, "url").unwrap_or_default();
                        if !(url.starts_with("https://") || url.starts_with("http://"))
                            || url.len() > 512
                        {
                            return Err("a link button needs an http(s) url".into());
                        }
                        button["url"] = Value::String(url.into());
                    } else {
                        let id = check_custom_id(item)?;
                        if !custom_ids.insert(id.clone()) {
                            return Err(format!("custom_id '{id}' is used twice"));
                        }
                        button["custom_id"] = Value::String(id);
                    }
                    out_items.push(button);
                }
                Some("select") => {
                    if items.len() != 1 {
                        return Err("a select menu takes a whole row".into());
                    }
                    let id = check_custom_id(item)?;
                    if !custom_ids.insert(id.clone()) {
                        return Err(format!("custom_id '{id}' is used twice"));
                    }
                    let options = item
                        .get("options")
                        .and_then(Value::as_array)
                        .ok_or("a select menu needs options")?;
                    if options.is_empty() || options.len() > 25 {
                        return Err("a select menu has 1-25 options".into());
                    }
                    let mut values = std::collections::HashSet::new();
                    let mut out_options = Vec::new();
                    for option in options {
                        let option = option
                            .as_object()
                            .ok_or("a select option must be an object")?;
                        let label = text_field(option, "label").unwrap_or_default();
                        let value = text_field(option, "value").unwrap_or_default();
                        if !(1..=100).contains(&label.chars().count())
                            || !(1..=100).contains(&value.chars().count())
                            || !values.insert(value.to_string())
                        {
                            return Err(
                                "each select option needs a label and a unique value".into()
                            );
                        }
                        let mut out = serde_json::json!({ "label": label, "value": value });
                        if let Some(description) = text_field(option, "description") {
                            out["description"] =
                                Value::String(description.chars().take(100).collect());
                        }
                        if let Some(emoji) = option.get("emoji").filter(|e| !e.is_null()) {
                            out["emoji"] = emoji.clone();
                        }
                        if option.get("default").and_then(Value::as_bool) == Some(true) {
                            out["default"] = Value::Bool(true);
                        }
                        out_options.push(out);
                    }
                    let min = item.get("min_values").and_then(Value::as_u64).unwrap_or(1);
                    let max = item.get("max_values").and_then(Value::as_u64).unwrap_or(1);
                    if min > max || max as usize > out_options.len() || max == 0 {
                        return Err("select menu min/max values don't fit its options".into());
                    }
                    let mut select = serde_json::json!({
                        "type": "select",
                        "custom_id": id,
                        "options": out_options,
                        "min_values": min,
                        "max_values": max,
                        "disabled": item.get("disabled").and_then(Value::as_bool).unwrap_or(false),
                    });
                    if let Some(placeholder) = text_field(item, "placeholder") {
                        select["placeholder"] =
                            Value::String(placeholder.chars().take(150).collect());
                    }
                    out_items.push(select);
                }
                _ => return Err("unknown component type".into()),
            }
        }
        out_rows.push(serde_json::json!({ "type": "action_row", "components": out_items }));
    }
    Ok(Value::Array(out_rows))
}

/// Finds the component `custom_id` in validated `components`, returning its
/// type word and, for a select menu, its allowed values and bounds.
pub fn find_component<'a>(components: &'a Value, custom_id: &str) -> Option<&'a Value> {
    components
        .as_array()?
        .iter()
        .filter_map(|row| row.get("components")?.as_array())
        .flatten()
        .find(|c| c.get("custom_id").and_then(Value::as_str) == Some(custom_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn play() -> CommandDefinition {
        serde_json::from_value::<CommandDefinition>(json!({
            "name": "play",
            "description": "Play a song",
            "options": [
                {"name": "query", "description": "Song or URL", "type": 3, "required": true},
                {"name": "volume", "description": "Volume", "type": "integer", "min_value": 0, "max_value": 200}
            ]
        }))
        .unwrap()
        .validate()
        .unwrap()
    }

    #[test]
    fn definitions_normalise_types_and_check_names() {
        let def = play();
        assert_eq!(def.options[0].option_type, json!("string"));
        let bad = |value: Value| {
            serde_json::from_value::<CommandDefinition>(value)
                .unwrap()
                .validate()
                .is_err()
        };
        assert!(bad(json!({"name": "Play", "description": "x"})));
        assert!(bad(json!({"name": "play", "description": ""})));
        assert!(bad(json!({"name": "p", "description": "x", "options": [
            {"name": "a", "description": "x", "type": "string"},
            {"name": "b", "description": "x", "type": "string", "required": true}
        ]})));
        assert!(bad(json!({"name": "p", "description": "x", "options": [
            {"name": "a", "description": "x", "type": "boolean", "choices": [{"name": "y", "value": true}]}
        ]})));
    }

    #[test]
    fn invocations_are_checked_against_the_definition() {
        let def = play();
        let ok = resolve_options(
            &def,
            &[
                OptionValue {
                    name: "query".into(),
                    value: json!("daft punk"),
                },
                OptionValue {
                    name: "volume".into(),
                    value: json!("80"),
                },
            ],
        )
        .unwrap();
        assert_eq!(ok[1].value, json!(80));
        assert_eq!(ok[1].option_type, "integer");

        let fails = |given: Vec<OptionValue>| resolve_options(&def, &given).is_err();
        assert!(fails(vec![]));
        assert!(fails(vec![OptionValue {
            name: "query".into(),
            value: json!("  ")
        }]));
        assert!(fails(vec![
            OptionValue {
                name: "query".into(),
                value: json!("x")
            },
            OptionValue {
                name: "volume".into(),
                value: json!(500)
            },
        ]));
        assert!(fails(vec![
            OptionValue {
                name: "query".into(),
                value: json!("x")
            },
            OptionValue {
                name: "nope".into(),
                value: json!(1)
            },
        ]));
    }

    #[test]
    fn components_are_validated_and_normalised() {
        let ok = validate_components(&json!([
            {"type": 1, "components": [
                {"type": 2, "style": 1, "label": "Pause", "custom_id": "pause"},
                {"type": "button", "style": "link", "label": "Source", "url": "https://example.test"}
            ]},
            {"type": "action_row", "components": [
                {"type": 3, "custom_id": "pick", "options": [
                    {"label": "One", "value": "1"}, {"label": "Two", "value": "2", "default": true}
                ], "max_values": 2}
            ]}
        ]))
        .unwrap();
        assert_eq!(ok[0]["components"][0]["style"], "primary");
        assert_eq!(ok[0]["components"][1]["url"], "https://example.test");
        assert_eq!(ok[1]["components"][0]["type"], "select");
        assert!(find_component(&ok, "pick").is_some());
        assert!(find_component(&ok, "missing").is_none());

        assert!(validate_components(&json!([{"type": 1, "components": [
            {"type": 2, "style": 1, "label": "A", "custom_id": "x"},
            {"type": 2, "style": 1, "label": "B", "custom_id": "x"}
        ]}]))
        .is_err());
        assert!(validate_components(&json!([{"type": 1, "components": [
            {"type": 2, "style": 5, "label": "A", "url": "javascript:alert(1)"}
        ]}]))
        .is_err());
        assert!(validate_components(&json!([{"type": 2, "label": "A"}])).is_err());
    }
}
