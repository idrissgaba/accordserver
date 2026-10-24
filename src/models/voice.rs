use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceState {
    pub user_id: String,
    pub space_id: Option<String>,
    pub channel_id: Option<String>,
    pub session_id: String,
    pub deaf: bool,
    pub mute: bool,
    pub self_deaf: bool,
    pub self_mute: bool,
    pub self_stream: bool,
    pub self_video: bool,
    pub suppress: bool,
    /// When this audience member asked to speak on a stage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_to_speak_timestamp: Option<String>,
}
