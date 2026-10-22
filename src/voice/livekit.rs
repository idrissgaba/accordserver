use crate::error::AppError;
use livekit_api::access_token::{AccessToken, VideoGrants};
use livekit_api::services::room::{CreateRoomOptions, RoomClient, UpdateParticipantOptions};
use std::sync::Arc;

use crate::voice::moderation::MediaPolicy;

#[derive(Clone)]
pub struct LiveKitClient {
    internal_url: String,
    external_url: String,
    api_key: String,
    api_secret: String,
    room_client: Arc<RoomClient>,
}

impl LiveKitClient {
    pub fn new(internal_url: &str, external_url: &str, api_key: &str, api_secret: &str) -> Self {
        Self {
            internal_url: internal_url.to_string(),
            external_url: external_url.to_string(),
            api_key: api_key.to_string(),
            api_secret: api_secret.to_string(),
            room_client: Arc::new(RoomClient::with_api_key(internal_url, api_key, api_secret)),
        }
    }

    pub fn internal_url(&self) -> &str {
        &self.internal_url
    }

    pub fn external_url(&self) -> &str {
        &self.external_url
    }

    pub fn room_name(channel_id: &str) -> String {
        format!("channel_{channel_id}")
    }

    pub fn generate_token(
        &self,
        user_id: &str,
        display_name: &str,
        channel_id: &str,
    ) -> Result<String, AppError> {
        self.generate_token_with_policy(user_id, display_name, channel_id, &MediaPolicy::OPEN)
    }

    /// A join token whose grants carry the participant's voice-moderation
    /// policy (which sources they may publish, whether they may subscribe).
    pub fn generate_token_with_policy(
        &self,
        user_id: &str,
        display_name: &str,
        channel_id: &str,
        policy: &MediaPolicy,
    ) -> Result<String, AppError> {
        let room_name = Self::room_name(channel_id);
        AccessToken::with_api_key(&self.api_key, &self.api_secret)
            .with_ttl(std::time::Duration::from_secs(60))
            .with_identity(user_id)
            .with_name(display_name)
            .with_grants(VideoGrants {
                room_join: true,
                room: room_name,
                can_publish: policy.can_publish(),
                can_subscribe: policy.can_listen(),
                can_publish_data: true,
                can_publish_sources: policy.publish_source_names(),
                ..Default::default()
            })
            .to_jwt()
            .map_err(|e| AppError::Internal(format!("failed to generate livekit token: {}", e)))
    }

    /// Replaces a live participant's permission. False when LiveKit could not
    /// apply it (participant not in the room yet, LiveKit unreachable).
    pub async fn update_permission(
        &self,
        channel_id: &str,
        user_id: &str,
        permission: livekit_protocol::ParticipantPermission,
    ) -> bool {
        let room_name = Self::room_name(channel_id);
        match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.room_client.update_participant(
                &room_name,
                user_id,
                UpdateParticipantOptions {
                    permission: Some(permission),
                    ..Default::default()
                },
            ),
        )
        .await
        {
            Ok(Ok(_)) => true,
            result => {
                tracing::debug!(
                    "LiveKit permission update for {user_id} in {room_name} not applied: {result:?}"
                );
                false
            }
        }
    }

    /// Preflight connectivity check — called at startup to verify the server
    /// can reach LiveKit's Twirp API. Fails fast with a clear error instead of
    /// silently timing out on the first voice join.
    pub async fn check_connectivity(&self) -> Result<(), String> {
        // List rooms is a lightweight read-only Twirp call.
        match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.room_client.list_rooms(Vec::new()),
        )
        .await
        {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(e)) => Err(format!("LiveKit API error at {}: {}", self.internal_url, e)),
            Err(_) => Err(format!(
                "LiveKit unreachable at {} (timed out after 5s)",
                self.internal_url
            )),
        }
    }

    pub async fn ensure_room(&self, channel_id: &str) -> Result<(), AppError> {
        let room_name = Self::room_name(channel_id);
        self.room_client
            .create_room(
                &room_name,
                CreateRoomOptions {
                    empty_timeout: 300,
                    ..Default::default()
                },
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to create livekit room: {}", e)))?;
        Ok(())
    }

    pub async fn try_remove_participant(&self, channel_id: &str, user_id: &str) -> bool {
        let room_name = Self::room_name(channel_id);
        match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.room_client.remove_participant(&room_name, user_id),
        )
        .await
        {
            Ok(Ok(_)) => true,
            Ok(Err(livekit_api::services::ServiceError::Twirp(
                livekit_api::services::TwirpError::Twirp(error),
            ))) if error.code == "not_found" => true,
            result => {
                tracing::warn!(
                    "LiveKit eviction failed for {user_id} from {room_name}: {result:?}"
                );
                false
            }
        }
    }

    /// Subscribes `user_id` to every track the others in the room publish.
    /// LiveKit stops forwarding media when `can_subscribe` is revoked but does
    /// not resume it when the right comes back, so lifting a server deafen
    /// needs this. False when LiveKit could not do it.
    pub async fn resubscribe_all(&self, channel_id: &str, user_id: &str) -> bool {
        let room_name = Self::room_name(channel_id);
        let participants = match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.room_client.list_participants(&room_name),
        )
        .await
        {
            Ok(Ok(participants)) => participants,
            result => {
                tracing::debug!("LiveKit participants of {room_name} unavailable: {result:?}");
                return false;
            }
        };
        let track_sids: Vec<String> = participants
            .into_iter()
            .filter(|p| p.identity != user_id)
            .flat_map(|p| p.tracks.into_iter().map(|t| t.sid))
            .collect();
        if track_sids.is_empty() {
            return true;
        }
        match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.room_client
                .update_subscriptions(&room_name, user_id, track_sids, true),
        )
        .await
        {
            Ok(Ok(())) => true,
            result => {
                tracing::debug!(
                    "LiveKit resubscription of {user_id} in {room_name} failed: {result:?}"
                );
                false
            }
        }
    }

    pub async fn delete_room_if_empty(&self, channel_id: &str) {
        let room_name = Self::room_name(channel_id);
        match self.room_client.list_participants(&room_name).await {
            Ok(participants) => {
                if participants.is_empty() {
                    if let Err(e) = self.room_client.delete_room(&room_name).await {
                        tracing::warn!("Failed to delete empty room {}: {}", room_name, e);
                    } else {
                        tracing::debug!("Deleted empty LiveKit room {}", room_name);
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Failed to list participants for room {}: {}", room_name, e);
            }
        }
    }
}
