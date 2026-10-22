use serde::Deserialize;

/// Most answers a poll may offer.
pub const MAX_POLL_ANSWERS: usize = 10;
/// Longest question, in characters.
pub const MAX_POLL_QUESTION_CHARS: usize = 300;
/// Longest answer, in characters.
pub const MAX_POLL_ANSWER_CHARS: usize = 55;
/// Longest a poll may stay open (32 days).
pub const MAX_POLL_DURATION_HOURS: i64 = 768;
/// How long a poll stays open when the request names no duration.
pub const DEFAULT_POLL_DURATION_HOURS: i64 = 24;

#[derive(Debug, Deserialize)]
pub struct CreatePollAnswer {
    pub text: String,
    #[serde(default)]
    pub emoji: Option<String>,
}

/// `POST /channels/{channel_id}/polls`: a new message carrying a poll.
#[derive(Debug, Deserialize)]
pub struct CreatePoll {
    pub question: String,
    pub answers: Vec<CreatePollAnswer>,
    #[serde(default)]
    pub allow_multiselect: bool,
    /// Hours until the poll closes (1..=768, default 24).
    #[serde(default)]
    pub duration_hours: Option<i64>,
    /// Optional message text shown above the poll.
    #[serde(default)]
    pub content: Option<String>,
}

impl CreatePoll {
    /// The poll's duration in hours once validated.
    pub fn duration(&self) -> i64 {
        self.duration_hours.unwrap_or(DEFAULT_POLL_DURATION_HOURS)
    }

    /// Checks the limits above; the error is safe to show the user.
    pub fn validate(&self) -> Result<(), String> {
        let question = self.question.trim();
        if question.is_empty() {
            return Err("poll question must not be empty".into());
        }
        if question.chars().count() > MAX_POLL_QUESTION_CHARS {
            return Err(format!(
                "poll question must be at most {MAX_POLL_QUESTION_CHARS} characters"
            ));
        }
        if self.answers.is_empty() || self.answers.len() > MAX_POLL_ANSWERS {
            return Err(format!(
                "a poll needs between 1 and {MAX_POLL_ANSWERS} answers"
            ));
        }
        for answer in &self.answers {
            let text = answer.text.trim();
            if text.is_empty() {
                return Err("poll answers must not be empty".into());
            }
            if text.chars().count() > MAX_POLL_ANSWER_CHARS {
                return Err(format!(
                    "poll answers must be at most {MAX_POLL_ANSWER_CHARS} characters"
                ));
            }
            if let Some(emoji) = &answer.emoji {
                if emoji.chars().count() > 64 {
                    return Err("poll answer emoji is too long".into());
                }
            }
        }
        let hours = self.duration();
        if !(1..=MAX_POLL_DURATION_HOURS).contains(&hours) {
            return Err(format!(
                "poll duration must be between 1 and {MAX_POLL_DURATION_HOURS} hours"
            ));
        }
        if let Some(content) = &self.content {
            if content.chars().count() > 4000 {
                return Err("message content must be at most 4000 characters".into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn poll(question: &str, answers: &[&str], hours: Option<i64>) -> CreatePoll {
        CreatePoll {
            question: question.into(),
            answers: answers
                .iter()
                .map(|t| CreatePollAnswer {
                    text: (*t).into(),
                    emoji: None,
                })
                .collect(),
            allow_multiselect: false,
            duration_hours: hours,
            content: None,
        }
    }

    #[test]
    fn a_normal_poll_is_valid_and_defaults_to_a_day() {
        let p = poll("Pizza or sushi?", &["Pizza", "Sushi"], None);
        assert!(p.validate().is_ok());
        assert_eq!(p.duration(), 24);
    }

    #[test]
    fn rejects_empty_or_oversized_parts() {
        assert!(poll(" ", &["a"], None).validate().is_err());
        assert!(poll("q", &[], None).validate().is_err());
        assert!(poll("q", &[""], None).validate().is_err());
        assert!(poll("q", &["a"; 11], None).validate().is_err());
        assert!(poll(&"x".repeat(301), &["a"], None).validate().is_err());
        assert!(poll("q", &[&"x".repeat(56)], None).validate().is_err());
    }

    #[test]
    fn duration_must_stay_within_32_days() {
        assert!(poll("q", &["a"], Some(0)).validate().is_err());
        assert!(poll("q", &["a"], Some(769)).validate().is_err());
        assert!(poll("q", &["a"], Some(768)).validate().is_ok());
    }
}
