//! Stage channels: everyone joins in the audience, members raise a hand, and
//! stage moderators (`mute_members`) decide who speaks.
mod common;

use base64::Engine;
use common::{authenticated_json_request, parse_body, TestServer};
use http::{Method, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

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

async fn create_stage(server: &TestServer, space: &str, auth: &str) -> String {
    let (status, body) = send(
        server,
        Method::POST,
        &format!("/api/v1/spaces/{space}/channels"),
        auth,
        json!({ "name": "stage", "type": "stage" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["data"]["id"].as_str().unwrap().to_string()
}

async fn join(server: &TestServer, channel: &str, auth: &str) -> Value {
    let (status, body) = send(
        server,
        Method::POST,
        &format!("/api/v1/channels/{channel}/voice/join"),
        auth,
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["data"].clone()
}

async fn stage_update(
    server: &TestServer,
    space: &str,
    user: &str,
    auth: &str,
    body: Value,
) -> (StatusCode, Value) {
    send(
        server,
        Method::PATCH,
        &format!("/api/v1/spaces/{space}/voice-states/{user}"),
        auth,
        body,
    )
    .await
}

fn can_publish(token: &str) -> bool {
    let payload = token.split('.').nth(1).unwrap();
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .unwrap();
    serde_json::from_slice::<Value>(&bytes).unwrap()["video"]["canPublish"] == true
}

fn voice_state(server: &TestServer, user_id: &str) -> accordserver::models::voice::VoiceState {
    server.state.voice_states.get(user_id).unwrap().clone()
}

#[tokio::test]
async fn everyone_joins_a_stage_in_the_audience() {
    let server = TestServer::new().await;
    let owner = server.create_user_with_token("owner").await;
    let member = server.create_user_with_token("member").await;
    let space = server.create_space(&owner.user.id, "space").await;
    server.add_member(&space, &member.user.id).await;
    let stage = create_stage(&server, &space, &owner.auth_header()).await;

    for auth in [member.auth_header(), owner.auth_header()] {
        let joined = join(&server, &stage, &auth).await;
        assert_eq!(joined["voice_state"]["suppress"], true);
        assert!(!can_publish(joined["token"].as_str().unwrap()));
    }
    // A plain voice channel is not a stage.
    let voice = server.create_voice_channel(&space, "voice").await;
    join(&server, &voice, &member.auth_header()).await;
    let (status, _) = stage_update(
        &server,
        &space,
        "@me",
        &member.auth_header(),
        json!({ "request_to_speak": true }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_raised_hand_waits_for_a_moderator() {
    let server = TestServer::new().await;
    let owner = server.create_user_with_token("owner").await;
    let member = server.create_user_with_token("member").await;
    let other = server.create_user_with_token("other").await;
    let space = server.create_space(&owner.user.id, "space").await;
    server.add_member(&space, &member.user.id).await;
    server.add_member(&space, &other.user.id).await;
    let stage = create_stage(&server, &space, &owner.auth_header()).await;
    join(&server, &stage, &member.auth_header()).await;
    join(&server, &stage, &other.auth_header()).await;

    // Members cannot step up on their own, nor move anyone else.
    let (status, _) = stage_update(
        &server,
        &space,
        "@me",
        &member.auth_header(),
        json!({ "suppress": false }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = stage_update(
        &server,
        &space,
        &member.user.id,
        &other.auth_header(),
        json!({ "suppress": false }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, body) = stage_update(
        &server,
        &space,
        "@me",
        &member.auth_header(),
        json!({ "request_to_speak": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"]["request_to_speak_timestamp"].is_string());
    assert!(voice_state(&server, &member.user.id).suppress);

    // The moderator brings them up: the hand goes down.
    let (status, body) = stage_update(
        &server,
        &space,
        &member.user.id,
        &owner.auth_header(),
        json!({ "suppress": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["suppress"], false);
    assert!(body["data"].get("request_to_speak_timestamp").is_none());

    // And back to the audience.
    let (status, _) = stage_update(
        &server,
        &space,
        &member.user.id,
        &owner.auth_header(),
        json!({ "suppress": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(voice_state(&server, &member.user.id).suppress);

    // Anyone may leave the stage or lower their hand.
    stage_update(
        &server,
        &space,
        "@me",
        &other.auth_header(),
        json!({ "request_to_speak": true }),
    )
    .await;
    let (status, _) = stage_update(
        &server,
        &space,
        "@me",
        &other.auth_header(),
        json!({ "request_to_speak": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(voice_state(&server, &other.user.id)
        .request_to_speak_timestamp
        .is_none());
}

#[tokio::test]
async fn moderators_may_step_up_and_rejoining_starts_in_the_audience() {
    let server = TestServer::new().await;
    let owner = server.create_user_with_token("owner").await;
    let space = server.create_space(&owner.user.id, "space").await;
    let stage = create_stage(&server, &space, &owner.auth_header()).await;
    join(&server, &stage, &owner.auth_header()).await;

    let (status, body) = stage_update(
        &server,
        &space,
        "@me",
        &owner.auth_header(),
        json!({ "suppress": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["suppress"], false);

    let rejoined = join(&server, &stage, &owner.auth_header()).await;
    assert_eq!(rejoined["voice_state"]["suppress"], true);
}
