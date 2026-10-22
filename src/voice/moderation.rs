//! Voice moderation: what a participant may send and receive, enforced in
//! LiveKit rather than trusted to the client.
//!
//! A member's media policy comes from their space membership (server mute and
//! deafen, set by moderators) and from the channel's permissions (`speak`,
//! `stream`). It is baked into the LiveKit token at join time and re-applied
//! to the live participant whenever it changes, so revoking a right takes
//! effect mid-call: LiveKit unpublishes tracks the participant may no longer
//! send and drops subscriptions when they may no longer listen.

use livekit_protocol::{ParticipantPermission, TrackSource};

use crate::db;
use crate::error::AppError;
use crate::gateway::events::GatewayBroadcast;
use crate::middleware::auth::AuthUser;
use crate::middleware::permissions::{
    require_channel_permission, require_permission, resolve_channel_permissions,
};
use crate::models::channel::ChannelRow;
use crate::models::permission::has_permission;
use crate::models::voice::VoiceState;
use crate::state::AppState;

/// What one participant may do in a voice channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MediaPolicy {
    /// Server-muted by a moderator (the member's `mute`).
    pub mute: bool,
    /// Server-deafened by a moderator (the member's `deaf`). A deafened member
    /// can neither hear nor be heard.
    pub deaf: bool,
    /// The channel denies the member `speak`.
    pub suppress: bool,
    /// The channel allows the member `stream` (camera and screen share).
    pub stream: bool,
}

impl MediaPolicy {
    /// No restriction: DM calls, and members with every voice right.
    pub const OPEN: Self = Self {
        mute: false,
        deaf: false,
        suppress: false,
        stream: true,
    };

    pub fn can_speak(&self) -> bool {
        !(self.mute || self.deaf || self.suppress)
    }

    pub fn can_listen(&self) -> bool {
        !self.deaf
    }

    /// The LiveKit sources the participant may publish, or `None` when nothing
    /// is restricted (the token then keeps LiveKit's default: every source).
    pub fn publish_sources(&self) -> Option<Vec<TrackSource>> {
        if self.can_speak() && self.stream {
            return None;
        }
        let mut sources = Vec::new();
        if self.can_speak() {
            sources.push(TrackSource::Microphone);
        }
        if self.stream {
            sources.extend([
                TrackSource::Camera,
                TrackSource::ScreenShare,
                TrackSource::ScreenShareAudio,
            ]);
        }
        Some(sources)
    }

    /// Whether the participant may publish anything at all.
    pub fn can_publish(&self) -> bool {
        self.publish_sources()
            .is_none_or(|sources| !sources.is_empty())
    }

    /// Source names as LiveKit tokens spell them (`microphone`, `camera`...).
    pub fn publish_source_names(&self) -> Vec<String> {
        self.publish_sources()
            .unwrap_or_default()
            .into_iter()
            .map(source_name)
            .collect()
    }

    /// The permission to push to a live participant. Every field is set:
    /// LiveKit replaces the participant's whole permission with this one.
    pub fn permission(&self) -> ParticipantPermission {
        ParticipantPermission {
            can_subscribe: self.can_listen(),
            can_publish: self.can_publish(),
            can_publish_data: true,
            can_publish_sources: self
                .publish_sources()
                .unwrap_or_default()
                .into_iter()
                .map(|source| source as i32)
                .collect(),
            ..Default::default()
        }
    }

    /// Whether moving from `self` to `next` changes what LiveKit enforces
    /// (as opposed to only the flags shown to other members).
    fn livekit_differs(&self, next: &Self) -> bool {
        self.permission() != next.permission()
    }
}

fn source_name(source: TrackSource) -> String {
    match source {
        TrackSource::Camera => "camera",
        TrackSource::Microphone => "microphone",
        TrackSource::ScreenShare => "screen_share",
        TrackSource::ScreenShareAudio => "screen_share_audio",
        TrackSource::Unknown => "unknown",
    }
    .to_string()
}

/// The synthetic identity used to evaluate another user's channel rights.
async fn auth_for(state: &AppState, user_id: &str) -> Option<AuthUser> {
    let user = db::users::get_user(&state.db, user_id).await.ok()?;
    if user.disabled {
        return None;
    }
    Some(AuthUser {
        user_id: user_id.to_string(),
        is_bot: user.bot,
        is_admin: user.is_admin,
        is_guest: false,
        guest_space_id: None,
    })
}

/// The media policy of `user_id` in the space channel `channel_id`. Rights
/// that cannot be resolved are denied.
pub async fn media_policy(
    state: &AppState,
    space_id: &str,
    channel_id: &str,
    user_id: &str,
) -> MediaPolicy {
    let (mute, deaf) = match db::members::get_member_row(&state.db, space_id, user_id).await {
        Ok(row) => (row.mute, row.deaf),
        Err(_) => (false, false),
    };
    let is_admin = auth_for(state, user_id)
        .await
        .is_some_and(|auth| auth.is_admin);
    let (speak, stream) = if is_admin {
        (true, true)
    } else {
        match resolve_channel_permissions(&state.db, channel_id, space_id, user_id).await {
            Ok(perms) => (
                has_permission(&perms, "speak"),
                has_permission(&perms, "stream"),
            ),
            Err(_) => (false, false),
        }
    };
    MediaPolicy {
        mute,
        deaf,
        suppress: !speak,
        stream,
    }
}

/// The policy for a session that is joining `channel_id` (`None` space for DM
/// calls, which are never restricted).
pub async fn policy_for_join(
    state: &AppState,
    space_id: Option<&str>,
    channel_id: &str,
    user_id: &str,
) -> MediaPolicy {
    match space_id {
        Some(space_id) => media_policy(state, space_id, channel_id, user_id).await,
        None => MediaPolicy::OPEN,
    }
}

/// Records the policy a freshly joined session's token was issued with and
/// writes its flags into the voice state. Returns the state to announce.
pub fn record_join(state: &AppState, joined: VoiceState, policy: MediaPolicy) -> VoiceState {
    state
        .security
        .voice_media
        .insert(joined.session_id.clone(), policy);
    set_flags(state, &joined, &policy).unwrap_or(joined)
}

/// Writes the server flags into the user's voice state, if it is still the
/// session `current` describes. Returns the updated state.
fn set_flags(state: &AppState, current: &VoiceState, policy: &MediaPolicy) -> Option<VoiceState> {
    let mut entry = state.voice_states.get_mut(&current.user_id)?;
    let vs = entry.value_mut();
    if vs.session_id != current.session_id {
        return None;
    }
    vs.mute = policy.mute;
    vs.deaf = policy.deaf;
    vs.suppress = policy.suppress;
    Some(vs.clone())
}

/// Brings one live session in line with its current policy: announces changed
/// flags and pushes changed rights to LiveKit. A LiveKit update that fails
/// (the participant hasn't connected yet, LiveKit is unreachable) is retried
/// by the next reconciliation pass.
pub async fn sync_session(state: &AppState, current: &VoiceState) {
    let (Some(space_id), Some(channel_id)) =
        (current.space_id.as_deref(), current.channel_id.as_deref())
    else {
        return;
    };
    let policy = media_policy(state, space_id, channel_id, &current.user_id).await;

    let flags_changed = current.mute != policy.mute
        || current.deaf != policy.deaf
        || current.suppress != policy.suppress;
    if flags_changed {
        if let Some(updated) = set_flags(state, current, &policy) {
            crate::routes::voice::broadcast_voice_state_update(
                state,
                channel_id,
                Some(space_id),
                &updated,
            )
            .await;
        }
    }

    let applied = state
        .security
        .voice_media
        .get(&current.session_id)
        .map(|p| *p)
        .unwrap_or(MediaPolicy::OPEN);
    if applied == policy {
        return;
    }
    if applied.livekit_differs(&policy) && !state.test_mode {
        let Some(lk) = state.livekit_client.as_ref() else {
            return;
        };
        if !lk
            .update_permission(channel_id, &current.user_id, policy.permission())
            .await
        {
            return;
        }
    }
    state
        .security
        .voice_media
        .insert(current.session_id.clone(), policy);
}

/// Re-applies the policy of a member who may be in voice in `space_id`.
pub async fn refresh_member(state: &AppState, space_id: &str, user_id: &str) {
    let Some(current) = crate::voice::state::get_user_voice_state(state, user_id) else {
        return;
    };
    if current.space_id.as_deref() == Some(space_id) {
        sync_session(state, &current).await;
    }
}

/// Forgets policies of sessions that no longer exist.
pub fn prune(state: &AppState) {
    state.security.voice_media.retain(|session_id, _| {
        state
            .voice_states
            .iter()
            .any(|vs| vs.value().session_id == *session_id)
    });
}

/// Refuses a join into a full channel. Members who can move others may still
/// join, as in a moderator stepping into a capped room.
pub async fn require_room(
    state: &AppState,
    channel: &ChannelRow,
    auth: &AuthUser,
) -> Result<(), AppError> {
    let Some(limit) = channel.user_limit.filter(|limit| *limit > 0) else {
        return Ok(());
    };
    let others = crate::voice::state::get_channel_voice_states(state, &channel.id)
        .into_iter()
        .filter(|vs| vs.user_id != auth.user_id)
        .count() as i64;
    if others < limit {
        return Ok(());
    }
    if let Some(space_id) = channel.space_id.as_deref() {
        if require_permission(&state.db, space_id, auth, "move_members")
            .await
            .is_ok()
        {
            return Ok(());
        }
    }
    Err(AppError::Forbidden("channel_full".into()))
}

/// Moves a member who is in voice in `space_id` to the voice channel
/// `channel_id` of the same space, as a moderator dragging them across.
///
/// The server updates the member's voice state, evicts them from the old
/// LiveKit room and sends their sessions a `voice.server_update` for the new
/// room with `"moved": true`; a client that understands it follows the move.
pub async fn move_member(
    state: &AppState,
    space_id: &str,
    user_id: &str,
    channel_id: &str,
) -> Result<VoiceState, AppError> {
    let current = crate::voice::state::get_user_voice_state(state, user_id)
        .filter(|vs| vs.space_id.as_deref() == Some(space_id))
        .ok_or_else(|| AppError::BadRequest("target user is not connected to voice".into()))?;
    let from = current.channel_id.clone().unwrap_or_default();
    if from == channel_id {
        return Ok(current);
    }

    let channel = db::channels::get_channel_row(&state.db, channel_id).await?;
    if channel.space_id.as_deref() != Some(space_id) {
        return Err(AppError::NotFound("channel not found in this space".into()));
    }
    if channel.channel_type != "voice" {
        return Err(AppError::BadRequest("channel_not_voice".into()));
    }
    let target = auth_for(state, user_id)
        .await
        .ok_or_else(|| AppError::NotFound("user not found".into()))?;
    require_channel_permission(&state.db, channel_id, &target, "connect")
        .await
        .map_err(|_| AppError::Forbidden("target user cannot connect to that channel".into()))?;

    let session_id = crate::snowflake::generate();
    let (moved, _) = crate::voice::state::join_voice_channel(
        state,
        user_id,
        Some(space_id),
        channel_id,
        &session_id,
        current.self_mute,
        current.self_deaf,
        false,
        false,
    );
    let policy = media_policy(state, space_id, channel_id, user_id).await;
    let moved = record_join(state, moved, policy);

    if !state.test_mode {
        if let Some(lk) = state.livekit_client.as_ref() {
            lk.ensure_room(channel_id).await?;
        }
    }

    crate::routes::voice::broadcast_voice_state_update(state, channel_id, Some(space_id), &moved)
        .await;

    // Tell the member's client where to go before pulling it out of the old
    // room, so it can follow the move instead of reconnecting to where it was.
    if let Some(lk) = state.livekit_client.as_ref() {
        let user = db::users::get_user(&state.db, user_id).await?;
        let display_name = user.display_name.as_deref().unwrap_or(&user.username);
        let token = lk.generate_token_with_policy(user_id, display_name, channel_id, &policy)?;
        let event = serde_json::json!({
            "op": 0,
            "type": "voice.server_update",
            "data": {
                "space_id": space_id,
                "channel_id": channel_id,
                "from_channel_id": from,
                "moved": true,
                "backend": "livekit",
                "url": lk.external_url(),
                "token": token
            }
        });
        if let Some(ref tx) = *state.gateway_tx.read().await {
            let _ = tx.send(GatewayBroadcast {
                space_id: None,
                target_user_ids: Some(vec![user_id.to_string()]),
                event,
                intent: "voice_states".to_string(),
                required_permission: None,
            });
        }
        if !state.test_mode {
            if let Err(err) = crate::security::evict_voice_participant(state, &from, user_id).await
            {
                tracing::warn!("voice eviction could not be queued: {err}");
            }
            lk.delete_room_if_empty(&from).await;
        }
    }
    Ok(moved)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(mute: bool, deaf: bool, suppress: bool, stream: bool) -> MediaPolicy {
        MediaPolicy {
            mute,
            deaf,
            suppress,
            stream,
        }
    }

    #[test]
    fn open_policy_restricts_nothing() {
        let open = MediaPolicy::OPEN;
        assert!(open.can_speak() && open.can_listen() && open.can_publish());
        assert_eq!(open.publish_sources(), None);
        let permission = open.permission();
        assert!(permission.can_publish && permission.can_subscribe && permission.can_publish_data);
        assert!(permission.can_publish_sources.is_empty());
    }

    #[test]
    fn muted_member_keeps_video_but_loses_the_microphone() {
        let muted = policy(true, false, false, true);
        assert_eq!(
            muted.publish_source_names(),
            ["camera", "screen_share", "screen_share_audio"]
        );
        assert!(muted.can_publish() && muted.can_listen());
    }

    #[test]
    fn deafened_member_can_neither_hear_nor_speak() {
        let deaf = policy(false, true, false, true);
        assert!(!deaf.can_speak() && !deaf.can_listen());
        assert!(!deaf.permission().can_subscribe);
        assert!(!deaf
            .publish_source_names()
            .contains(&"microphone".to_string()));
    }

    #[test]
    fn no_stream_right_keeps_only_the_microphone() {
        assert_eq!(
            policy(false, false, false, false).publish_source_names(),
            ["microphone"]
        );
    }

    #[test]
    fn nothing_publishable_revokes_publishing() {
        let silent = policy(false, false, true, false);
        assert!(!silent.can_publish());
        let permission = silent.permission();
        assert!(!permission.can_publish && permission.can_publish_sources.is_empty());
    }

    #[test]
    fn flag_only_changes_do_not_touch_livekit() {
        // Muted for lack of `speak`, then server-muted as well: the same
        // sources either way.
        let suppressed = policy(false, false, true, true);
        let both = policy(true, false, true, true);
        assert!(!suppressed.livekit_differs(&both));
        assert!(MediaPolicy::OPEN.livekit_differs(&both));
    }
}
