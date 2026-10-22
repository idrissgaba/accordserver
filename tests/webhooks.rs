//! Incoming webhooks: management behind `manage_webhooks`, execution by secret
//! URL with the payload chat-service integrations already send.
mod common;

use common::{authenticated_json_request, authenticated_request, TestServer};
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

async fn call(server: &TestServer, request: Request<axum::body::Body>) -> (StatusCode, Value) {
    let response = server.router().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, body)
}

/// An unauthenticated JSON request, as an integration sends it.
fn anonymous(method: Method, uri: &str, body: &Value) -> Request<axum::body::Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

struct Setup {
    server: TestServer,
    owner: common::TestUser,
    space: String,
    channel: String,
}

async fn setup() -> Setup {
    let server = TestServer::new().await;
    let owner = server.create_user_with_token("owner").await;
    let space = server.create_space(&owner.user.id, "space").await;
    let channel = server.create_channel(&space, "alerts").await;
    Setup {
        server,
        owner,
        space,
        channel,
    }
}

async fn create_webhook(s: &Setup, name: &str) -> Value {
    let (status, body) = call(
        &s.server,
        authenticated_json_request(
            Method::POST,
            &format!("/api/v1/channels/{}/webhooks", s.channel),
            &s.owner.auth_header(),
            &json!({ "name": name }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["data"].clone()
}

fn execute_uri(hook: &Value) -> String {
    format!(
        "/api/v1/webhooks/{}/{}",
        hook["id"].as_str().unwrap(),
        hook["token"].as_str().unwrap()
    )
}

#[tokio::test]
async fn creating_a_webhook_needs_manage_webhooks() {
    let s = setup().await;
    let member = s.server.create_user_with_token("member").await;
    s.server.add_member(&s.space, &member.user.id).await;
    let (status, _) = call(
        &s.server,
        authenticated_json_request(
            Method::POST,
            &format!("/api/v1/channels/{}/webhooks", s.channel),
            &member.auth_header(),
            &json!({ "name": "Sonarr" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let hook = create_webhook(&s, "  Sonarr  ").await;
    assert_eq!(hook["name"], "Sonarr");
    assert_eq!(hook["channel_id"], s.channel.as_str());
    assert!(hook["token"].as_str().is_some_and(|t| t.len() == 68));
}

#[tokio::test]
async fn webhooks_only_post_in_space_text_channels() {
    let s = setup().await;
    let voice = s.server.create_voice_channel(&s.space, "voice").await;
    let (status, _) = call(
        &s.server,
        authenticated_json_request(
            Method::POST,
            &format!("/api/v1/channels/{voice}/webhooks"),
            &s.owner.auth_header(),
            &json!({ "name": "x" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let friend = s.server.create_user_with_token("friend").await;
    let dm = s.server.create_dm(&s.owner.user.id, &friend.user.id).await;
    let (status, _) = call(
        &s.server,
        authenticated_json_request(
            Method::POST,
            &format!("/api/v1/channels/{dm}/webhooks"),
            &s.owner.auth_header(),
            &json!({ "name": "x" }),
        ),
    )
    .await;
    assert!(status.is_client_error(), "got {status}");
}

#[tokio::test]
async fn executing_posts_a_message_as_the_webhook() {
    let s = setup().await;
    let hook = create_webhook(&s, "Uptime").await;
    let mut rx = s
        .server
        .state
        .gateway_tx
        .read()
        .await
        .as_ref()
        .unwrap()
        .subscribe();

    // No wait: an empty 204, like the services expect.
    let (status, body) = call(
        &s.server,
        anonymous(
            Method::POST,
            &execute_uri(&hook),
            &json!({ "content": "Website is down" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(body, Value::Null);
    let event = rx.try_recv().unwrap();
    assert_eq!(event.event["type"], "message.create");
    assert_eq!(event.event["data"]["webhook_id"], hook["id"]);
    assert_eq!(event.event["data"]["author_id"], hook["user_id"]);

    // wait=true answers with the message, and a per-message name shows.
    let (status, message) = call(
        &s.server,
        anonymous(
            Method::POST,
            &format!("{}?wait=true", execute_uri(&hook)),
            &json!({
                "content": "Back up",
                "username": "Uptime Kuma",
                "avatar_url": "https://example.test/a.png",
                "embeds": [{"title": "Monitor up", "color": 3066993, "footer": {}}]
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{message}");
    assert_eq!(message["content"], "Back up");
    assert_eq!(message["webhook_author"]["name"], "Uptime Kuma");
    assert_eq!(message["embeds"][0]["title"], "Monitor up");
    assert!(message["embeds"][0]["footer"].is_null());

    // The name override survives a reload of the channel.
    let (_, listed) = call(
        &s.server,
        authenticated_request(
            Method::GET,
            &format!("/api/v1/channels/{}/messages", s.channel),
            &s.owner.auth_header(),
        ),
    )
    .await;
    let names: Vec<&Value> = listed["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| &m["webhook_author"]["name"])
        .collect();
    assert!(names.contains(&&json!("Uptime Kuma")));
}

#[tokio::test]
async fn a_wrong_token_or_an_empty_message_is_refused() {
    let s = setup().await;
    let hook = create_webhook(&s, "Hook").await;
    let wrong = format!(
        "/api/v1/webhooks/{}/{}",
        hook["id"].as_str().unwrap(),
        "x".repeat(68)
    );
    let (status, _) = call(
        &s.server,
        anonymous(Method::POST, &wrong, &json!({"content": "hi"})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(
        &s.server,
        anonymous(
            Method::POST,
            "/api/v1/webhooks/123/abc",
            &json!({"content": "hi"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(
        &s.server,
        anonymous(
            Method::POST,
            &execute_uri(&hook),
            &json!({"content": "   "}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call(
        &s.server,
        anonymous(
            Method::POST,
            &execute_uri(&hook),
            &json!({"content": "x".repeat(4001)}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // GET on the URL lets an integration check it, without the secret.
    let (status, info) = call(
        &s.server,
        anonymous(Method::GET, &execute_uri(&hook), &Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(info["name"], "Hook");
    assert!(info.get("token").is_none());
}

#[tokio::test]
async fn everyone_pings_need_an_explicit_opt_in() {
    let s = setup().await;
    let hook = create_webhook(&s, "Hook").await;
    let uri = format!("{}?wait=true", execute_uri(&hook));
    let (_, quiet) = call(
        &s.server,
        anonymous(Method::POST, &uri, &json!({"content": "@everyone hi"})),
    )
    .await;
    assert_eq!(quiet["mention_everyone"], false);
    let (_, loud) = call(
        &s.server,
        anonymous(
            Method::POST,
            &uri,
            &json!({"content": "@everyone hi", "allowed_mentions": {"parse": ["everyone"]}}),
        ),
    )
    .await;
    assert_eq!(loud["mention_everyone"], true);
}

#[tokio::test]
async fn a_webhook_edits_and_deletes_only_its_own_messages() {
    let s = setup().await;
    let hook = create_webhook(&s, "Hook").await;
    let (_, message) = call(
        &s.server,
        anonymous(
            Method::POST,
            &format!("{}?wait=true", execute_uri(&hook)),
            &json!({"content": "first"}),
        ),
    )
    .await;
    let message_uri = format!(
        "{}/messages/{}",
        execute_uri(&hook),
        message["id"].as_str().unwrap()
    );
    let (status, edited) = call(
        &s.server,
        anonymous(Method::PATCH, &message_uri, &json!({"content": "edited"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(edited["content"], "edited");

    // A member's message is out of reach.
    let (_, posted) = call(
        &s.server,
        authenticated_json_request(
            Method::POST,
            &format!("/api/v1/channels/{}/messages", s.channel),
            &s.owner.auth_header(),
            &json!({"content": "mine"}),
        ),
    )
    .await;
    let other_uri = format!(
        "{}/messages/{}",
        execute_uri(&hook),
        posted["data"]["id"].as_str().unwrap()
    );
    let (status, _) = call(
        &s.server,
        anonymous(Method::DELETE, &other_uri, &Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = call(
        &s.server,
        anonymous(Method::DELETE, &message_uri, &Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = call(
        &s.server,
        authenticated_request(
            Method::GET,
            &format!(
                "/api/v1/channels/{}/messages/{}",
                s.channel,
                message["id"].as_str().unwrap()
            ),
            &s.owner.auth_header(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn managers_list_rename_move_and_delete_webhooks() {
    let s = setup().await;
    let hook = create_webhook(&s, "Hook").await;
    let other = s.server.create_channel(&s.space, "general").await;
    let id = hook["id"].as_str().unwrap();

    let (_, listed) = call(
        &s.server,
        authenticated_request(
            Method::GET,
            &format!("/api/v1/spaces/{}/webhooks", s.space),
            &s.owner.auth_header(),
        ),
    )
    .await;
    assert_eq!(listed["data"].as_array().unwrap().len(), 1);

    let (status, updated) = call(
        &s.server,
        authenticated_json_request(
            Method::PATCH,
            &format!("/api/v1/webhooks/{id}"),
            &s.owner.auth_header(),
            &json!({"name": "Renamed", "channel_id": other}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["data"]["name"], "Renamed");
    assert_eq!(updated["data"]["channel_id"], other.as_str());
    // Its bot user carries the new name.
    let user =
        accordserver::db::users::get_user(s.server.pool(), hook["user_id"].as_str().unwrap())
            .await
            .unwrap();
    assert_eq!(user.display_name.as_deref(), Some("Renamed"));
    assert!(user.bot);

    // Posts now land in the new channel.
    let (_, message) = call(
        &s.server,
        anonymous(
            Method::POST,
            &format!("{}?wait=true", execute_uri(&hook)),
            &json!({"content": "moved"}),
        ),
    )
    .await;
    assert_eq!(message["channel_id"], other.as_str());

    let (status, _) = call(
        &s.server,
        authenticated_request(
            Method::DELETE,
            &format!("/api/v1/webhooks/{id}"),
            &s.owner.auth_header(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(
        &s.server,
        anonymous(
            Method::POST,
            &execute_uri(&hook),
            &json!({"content": "gone"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // Past messages keep their author.
    assert!(
        accordserver::db::users::get_user(s.server.pool(), hook["user_id"].as_str().unwrap())
            .await
            .is_ok()
    );
}
