//! Voice moderation is enforced in LiveKit, not left to the client: server mute
//! and deafen, the `speak` and `stream` rights, moving and disconnecting
//! members, and channel user limits.
mod common;

use base64::Engine;
use common::{authenticated_json_request, parse_body, TestServer};
use http::{Method, StatusCode};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

/// Recorded LiveKit calls: (Twirp method, raw protobuf body).
type Calls = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

/// A stand-in for LiveKit's Twirp RoomService that records every call and
/// answers each with an empty message.
struct LiveKitMock {
    calls: Calls,
    task: tokio::task::JoinHandle<()>,
}

impl LiveKitMock {
    async fn start() -> (Self, String) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let recorded = calls.clone();
        let app = axum::Router::new().fallback(move |uri: http::Uri, body: axum::body::Bytes| {
            let recorded = recorded.clone();
            async move {
                let method = uri.path().rsplit('/').next().unwrap_or("").to_string();
                recorded.lock().unwrap().push((method, body.to_vec()));
                (StatusCode::OK, "")
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (Self { calls, task }, url)
    }

    /// How many `method` calls named both this channel's room and this user.
    fn count(&self, method: &str, channel_id: &str, user_id: &str) -> usize {
        let room = format!("channel_{channel_id}");
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(m, body)| {
                m == method && contains(body, room.as_bytes()) && contains(body, user_id.as_bytes())
            })
            .count()
    }
}

impl Drop for LiveKitMock {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

async fn server_with_livekit(url: &str) -> TestServer {
    let mut server = TestServer::new().await;
    server.state.test_mode = false;
    server.state.livekit_client = Some(accordserver::voice::livekit::LiveKitClient::new(
        url,
        url,
        "test",
        "mock-secret",
    ));
    server
}

/// Puts `user_id` in a voice channel the way a gateway join would, before
/// any moderation applied.
fn join(server: &TestServer, user_id: &str, space_id: &str, channel_id: &str) {
    accordserver::voice::state::join_voice_channel(
        &server.state,
        user_id,
        Some(space_id),
        channel_id,
        &format!("session-{user_id}"),
        false,
        false,
        false,
        false,
    );
}

async fn send(
    server: &TestServer,
    method: Method,
    uri: &str,
    auth: &str,
    body: Value,
) -> (StatusCode, Value) {
    let response = server
        .router()
        .oneshot(authenticated_json_request(method, uri, auth, &body))
        .await
        .unwrap();
    let status = response.status();
    (status, parse_body(response).await)
}

async fn patch_member(
    server: &TestServer,
    space: &str,
    user_id: &str,
    auth: &str,
    body: Value,
) -> (StatusCode, Value) {
    send(
        server,
        Method::PATCH,
        &format!("/api/v1/spaces/{space}/members/{user_id}"),
        auth,
        body,
    )
    .await
}

/// The `video` grant of a LiveKit JWT.
fn grants(token: &str) -> Value {
    let payload = token.split('.').nth(1).unwrap();
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .unwrap();
    serde_json::from_slice::<Value>(&bytes).unwrap()["video"].clone()
}

fn voice_state(server: &TestServer, user_id: &str) -> accordserver::models::voice::VoiceState {
    server.state.voice_states.get(user_id).unwrap().clone()
}

#[tokio::test]
async fn server_mute_reaches_a_call_in_progress() {
    let (mock, url) = LiveKitMock::start().await;
    let server = server_with_livekit(&url).await;
    let owner = server.create_user_with_token("owner").await;
    let member = server.create_user_with_token("member").await;
    let space = server.create_space(&owner.user.id, "space").await;
    server.add_member(&space, &member.user.id).await;
    let channel = server.create_voice_channel(&space, "voice").await;
    join(&server, &member.user.id, &space, &channel);

    let (status, _) = patch_member(
        &server,
        &space,
        &member.user.id,
        &owner.auth_header(),
        json!({ "mute": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(voice_state(&server, &member.user.id).mute);
    assert_eq!(
        mock.count("UpdateParticipant", &channel, &member.user.id),
        1
    );

    let (status, _) = patch_member(
        &server,
        &space,
        &member.user.id,
        &owner.auth_header(),
        json!({ "mute": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!voice_state(&server, &member.user.id).mute);
    assert_eq!(
        mock.count("UpdateParticipant", &channel, &member.user.id),
        2
    );
}

#[tokio::test]
async fn server_deafen_needs_the_permission() {
    let server = TestServer::new().await;
    let owner = server.create_user_with_token("owner").await;
    let member = server.create_user_with_token("member").await;
    let other = server.create_user_with_token("other").await;
    let space = server.create_space(&owner.user.id, "space").await;
    server.add_member(&space, &member.user.id).await;
    server.add_member(&space, &other.user.id).await;
    let channel = server.create_voice_channel(&space, "voice").await;
    join(&server, &member.user.id, &space, &channel);

    let (status, _) = patch_member(
        &server,
        &space,
        &member.user.id,
        &other.auth_header(),
        json!({ "deaf": true }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(!voice_state(&server, &member.user.id).deaf);

    let (status, _) = patch_member(
        &server,
        &space,
        &member.user.id,
        &owner.auth_header(),
        json!({ "deaf": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(voice_state(&server, &member.user.id).deaf);
}

#[tokio::test]
async fn joining_carries_the_members_policy_into_the_token() {
    let server = TestServer::new().await;
    let owner = server.create_user_with_token("owner").await;
    let member = server.create_user_with_token("member").await;
    let space = server.create_space(&owner.user.id, "space").await;
    server.add_member(&space, &member.user.id).await;
    let channel = server.create_voice_channel(&space, "voice").await;

    // Unrestricted: the token keeps LiveKit's defaults.
    let (status, body) = send(
        &server,
        Method::POST,
        &format!("/api/v1/channels/{channel}/voice/join"),
        &member.auth_header(),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let video = grants(body["data"]["token"].as_str().unwrap());
    assert_eq!(video["canSubscribe"], true);
    assert!(video["canPublishSources"]
        .as_array()
        .is_none_or(|s| s.is_empty()));

    // Server-muted while away: the next join is muted too.
    patch_member(
        &server,
        &space,
        &member.user.id,
        &owner.auth_header(),
        json!({ "mute": true }),
    )
    .await;
    let (status, body) = send(
        &server,
        Method::POST,
        &format!("/api/v1/channels/{channel}/voice/join"),
        &member.auth_header(),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["voice_state"]["mute"], true);
    let video = grants(body["data"]["token"].as_str().unwrap());
    assert_eq!(
        video["canPublishSources"],
        json!(["camera", "screen_share", "screen_share_audio"])
    );
    assert_eq!(video["canPublish"], true);
}

#[tokio::test]
async fn a_channel_that_denies_speak_suppresses_the_microphone() {
    let server = TestServer::new().await;
    let owner = server.create_user_with_token("owner").await;
    let member = server.create_user_with_token("member").await;
    let space = server.create_space(&owner.user.id, "space").await;
    server.add_member(&space, &member.user.id).await;
    let channel = server.create_voice_channel(&space, "stage").await;
    accordserver::db::permission_overwrites::upsert_overwrite(
        server.pool(),
        &channel,
        &accordserver::models::permission::PermissionOverwrite {
            id: member.user.id.clone(),
            overwrite_type: "member".into(),
            allow: vec![],
            deny: vec!["speak".into(), "stream".into()],
        },
    )
    .await
    .unwrap();

    let (status, body) = send(
        &server,
        Method::POST,
        &format!("/api/v1/channels/{channel}/voice/join"),
        &member.auth_header(),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["voice_state"]["suppress"], true);
    let video = grants(body["data"]["token"].as_str().unwrap());
    assert_eq!(video["canPublish"], false);
    assert_eq!(video["canSubscribe"], true);
}

#[tokio::test]
async fn revoking_a_right_mid_call_is_pushed_to_livekit() {
    let (mock, url) = LiveKitMock::start().await;
    let server = server_with_livekit(&url).await;
    let owner = server.create_user_with_token("owner").await;
    let member = server.create_user_with_token("member").await;
    let space = server.create_space(&owner.user.id, "space").await;
    server.add_member(&space, &member.user.id).await;
    let channel = server.create_voice_channel(&space, "voice").await;
    join(&server, &member.user.id, &space, &channel);

    accordserver::security::reconcile_voice_access(&server.state).await;
    assert_eq!(
        mock.count("UpdateParticipant", &channel, &member.user.id),
        0
    );

    accordserver::db::permission_overwrites::upsert_overwrite(
        server.pool(),
        &channel,
        &accordserver::models::permission::PermissionOverwrite {
            id: member.user.id.clone(),
            overwrite_type: "member".into(),
            allow: vec![],
            deny: vec!["stream".into()],
        },
    )
    .await
    .unwrap();
    accordserver::security::reconcile_voice_access(&server.state).await;
    assert_eq!(
        mock.count("UpdateParticipant", &channel, &member.user.id),
        1
    );
    // Applied once; later passes leave it alone.
    accordserver::security::reconcile_voice_access(&server.state).await;
    assert_eq!(
        mock.count("UpdateParticipant", &channel, &member.user.id),
        1
    );
    // The flags shown to others don't change for a video right.
    assert!(!voice_state(&server, &member.user.id).suppress);
}

#[tokio::test]
async fn moving_a_member_switches_their_channel_and_tells_their_client() {
    let (mock, url) = LiveKitMock::start().await;
    let server = server_with_livekit(&url).await;
    let owner = server.create_user_with_token("owner").await;
    let member = server.create_user_with_token("member").await;
    let space = server.create_space(&owner.user.id, "space").await;
    server.add_member(&space, &member.user.id).await;
    let from = server.create_voice_channel(&space, "lounge").await;
    let to = server.create_voice_channel(&space, "games").await;
    join(&server, &member.user.id, &space, &from);
    let mut rx = server
        .state
        .gateway_tx
        .read()
        .await
        .as_ref()
        .unwrap()
        .subscribe();

    let (status, _) = patch_member(
        &server,
        &space,
        &member.user.id,
        &owner.auth_header(),
        json!({ "channel_id": to }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        voice_state(&server, &member.user.id).channel_id.as_deref(),
        Some(to.as_str())
    );
    assert_eq!(mock.count("RemoveParticipant", &from, &member.user.id), 1);

    let mut announced = false;
    let mut told = false;
    while let Ok(broadcast) = rx.try_recv() {
        match broadcast.event["type"].as_str() {
            Some("voice.state_update") => {
                announced |= broadcast.event["data"]["channel_id"] == to.as_str();
            }
            Some("voice.server_update") => {
                assert_eq!(
                    broadcast.target_user_ids,
                    Some(vec![member.user.id.clone()])
                );
                let data = &broadcast.event["data"];
                assert_eq!(data["moved"], true);
                assert_eq!(data["from_channel_id"], from.as_str());
                assert_eq!(data["channel_id"], to.as_str());
                assert!(data["token"].as_str().is_some_and(|t| !t.is_empty()));
                told = true;
            }
            _ => {}
        }
    }
    assert!(announced && told);
}

#[tokio::test]
async fn moving_needs_move_members_and_a_member_in_voice() {
    let server = TestServer::new().await;
    let owner = server.create_user_with_token("owner").await;
    let member = server.create_user_with_token("member").await;
    let other = server.create_user_with_token("other").await;
    let space = server.create_space(&owner.user.id, "space").await;
    server.add_member(&space, &member.user.id).await;
    server.add_member(&space, &other.user.id).await;
    let from = server.create_voice_channel(&space, "lounge").await;
    let to = server.create_voice_channel(&space, "games").await;
    let text = server.create_channel(&space, "general").await;

    // Not in voice yet.
    let (status, _) = patch_member(
        &server,
        &space,
        &member.user.id,
        &owner.auth_header(),
        json!({ "channel_id": to }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    join(&server, &member.user.id, &space, &from);
    let (status, _) = patch_member(
        &server,
        &space,
        &member.user.id,
        &other.auth_header(),
        json!({ "channel_id": to }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = patch_member(
        &server,
        &space,
        &member.user.id,
        &owner.auth_header(),
        json!({ "channel_id": text }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        voice_state(&server, &member.user.id).channel_id.as_deref(),
        Some(from.as_str())
    );

    // A moderator role is enough.
    let moderator = server
        .create_role(&space, "mods", &["move_members", "connect", "view_channel"])
        .await;
    server.assign_role(&space, &other.user.id, &moderator).await;
    let (status, _) = patch_member(
        &server,
        &space,
        &member.user.id,
        &other.auth_header(),
        json!({ "channel_id": to }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn disconnecting_a_member_removes_them_from_voice() {
    let (mock, url) = LiveKitMock::start().await;
    let server = server_with_livekit(&url).await;
    let owner = server.create_user_with_token("owner").await;
    let member = server.create_user_with_token("member").await;
    let space = server.create_space(&owner.user.id, "space").await;
    server.add_member(&space, &member.user.id).await;
    let channel = server.create_voice_channel(&space, "voice").await;
    join(&server, &member.user.id, &space, &channel);

    let (status, _) = patch_member(
        &server,
        &space,
        &member.user.id,
        &owner.auth_header(),
        json!({ "channel_id": null }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!server.state.voice_states.contains_key(&member.user.id));
    assert_eq!(
        mock.count("RemoveParticipant", &channel, &member.user.id),
        1
    );
}

#[tokio::test]
async fn a_full_channel_refuses_joins_except_from_moderators() {
    let server = TestServer::new().await;
    let owner = server.create_user_with_token("owner").await;
    let member = server.create_user_with_token("member").await;
    let other = server.create_user_with_token("other").await;
    let space = server.create_space(&owner.user.id, "space").await;
    server.add_member(&space, &member.user.id).await;
    server.add_member(&space, &other.user.id).await;
    let channel = server.create_voice_channel(&space, "duo").await;
    sqlx::query(&accordserver::db::q(
        "UPDATE channels SET user_limit = 1 WHERE id = ?",
    ))
    .bind(&channel)
    .execute(server.pool())
    .await
    .unwrap();
    join(&server, &member.user.id, &space, &channel);

    let join_uri = format!("/api/v1/channels/{channel}/voice/join");
    let (status, body) = send(
        &server,
        Method::POST,
        &join_uri,
        &other.auth_header(),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"]["message"], "channel_full");

    // Rejoining your own seat is not blocked by the cap.
    let (status, _) = send(
        &server,
        Method::POST,
        &join_uri,
        &member.auth_header(),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // The owner can always step in.
    let (status, _) = send(
        &server,
        Method::POST,
        &join_uri,
        &owner.auth_header(),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}
