//! Slash commands and message components, end to end: a bot registers
//! commands, a member uses one, the bot is told over the gateway and answers
//! by token — with a message, a "thinking…" placeholder it fills in, an
//! ephemeral answer, or an update of the message whose button was clicked.
mod common;

use common::{authenticated_json_request, authenticated_request, TestServer, TestUser};
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use tokio::sync::broadcast::Receiver;
use tower::ServiceExt;

use accordserver::gateway::events::GatewayBroadcast;

async fn call(server: &TestServer, request: Request<axum::body::Body>) -> (StatusCode, Value) {
    let response = server.router().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

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
    owner: TestUser,
    bot: TestUser,
    member: TestUser,
    app_id: String,
    space: String,
    channel: String,
}

async fn setup() -> Setup {
    let server = TestServer::new().await;
    let (owner, bot) = server.create_bot_with_token("owner", "Music").await;
    let member = server.create_user_with_token("member").await;
    let space = server.create_space(&owner.user.id, "space").await;
    server.add_member(&space, &member.user.id).await;
    server.add_member(&space, &bot.user.id).await;
    let channel = server.create_channel(&space, "general").await;
    let app_id: String = sqlx::query_scalar(&accordserver::db::q(
        "SELECT id FROM applications WHERE bot_user_id = ?",
    ))
    .bind(&bot.user.id)
    .fetch_one(server.pool())
    .await
    .unwrap();
    let s = Setup {
        server,
        owner,
        bot,
        member,
        app_id,
        space,
        channel,
    };
    register(&s).await;
    s
}

async fn register(s: &Setup) {
    let (status, body) = call(
        &s.server,
        authenticated_json_request(
            Method::PUT,
            &format!("/api/v1/applications/{}/commands", s.app_id),
            &s.bot.auth_header(),
            &json!([
                {"name": "play", "description": "Play a song", "options": [
                    {"name": "query", "description": "Song or URL", "type": 3, "required": true}
                ]},
                {"name": "skip", "description": "Skip the song"}
            ]),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

async fn subscribe(server: &TestServer) -> Receiver<GatewayBroadcast> {
    server
        .state
        .gateway_tx
        .read()
        .await
        .as_ref()
        .unwrap()
        .subscribe()
}

/// The first `event_type` broadcast waiting in `rx`.
fn next_event(rx: &mut Receiver<GatewayBroadcast>, event_type: &str) -> Option<GatewayBroadcast> {
    while let Ok(b) = rx.try_recv() {
        if b.event["type"] == event_type {
            return Some(b);
        }
    }
    None
}

async fn command_id(s: &Setup, name: &str) -> String {
    let (_, body) = call(
        &s.server,
        authenticated_request(
            Method::GET,
            &format!("/api/v1/spaces/{}/commands", s.space),
            &s.member.auth_header(),
        ),
    )
    .await;
    body["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// Has the member use /play and returns the interaction the bot received.
async fn play(s: &Setup, rx: &mut Receiver<GatewayBroadcast>) -> Value {
    let id = command_id(s, "play").await;
    let (status, body) = call(
        &s.server,
        authenticated_json_request(
            Method::POST,
            "/api/v1/interactions",
            &s.member.auth_header(),
            &json!({
                "type": "application_command",
                "channel_id": s.channel,
                "command_id": id,
                "data": {"options": [{"name": "query", "value": "daft punk"}]}
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let delivered = next_event(rx, "interaction.create").expect("bot got no interaction");
    assert_eq!(delivered.target_user_ids, Some(vec![s.bot.user.id.clone()]));
    delivered.event["data"].clone()
}

fn callback_uri(interaction: &Value, path: &str) -> String {
    format!(
        "/api/v1/interactions/{}/{}/{path}",
        interaction["id"].as_str().unwrap(),
        interaction["token"].as_str().unwrap()
    )
}

async fn history(s: &Setup) -> Vec<Value> {
    let (_, body) = call(
        &s.server,
        authenticated_request(
            Method::GET,
            &format!("/api/v1/channels/{}/messages", s.channel),
            &s.member.auth_header(),
        ),
    )
    .await;
    body["data"].as_array().cloned().unwrap_or_default()
}

#[tokio::test]
async fn members_see_the_commands_of_bots_in_their_space() {
    let s = setup().await;
    let (status, body) = call(
        &s.server,
        authenticated_request(
            Method::GET,
            &format!("/api/v1/spaces/{}/commands", s.space),
            &s.member.auth_header(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let commands = body["data"].as_array().unwrap();
    assert_eq!(commands.len(), 2);
    let play = commands.iter().find(|c| c["name"] == "play").unwrap();
    assert_eq!(play["options"][0]["type"], "string");
    assert_eq!(play["application"]["bot_user_id"], s.bot.user.id.as_str());

    // Re-registering replaces the list.
    let (_, replaced) = call(
        &s.server,
        authenticated_json_request(
            Method::PUT,
            &format!("/api/v1/applications/{}/commands", s.app_id),
            &s.owner.auth_header(),
            &json!([{"name": "play", "description": "Play"}]),
        ),
    )
    .await;
    assert_eq!(replaced["data"].as_array().unwrap().len(), 1);

    // Somebody else's application is off limits.
    let (status, _) = call(
        &s.server,
        authenticated_json_request(
            Method::PUT,
            &format!("/api/v1/applications/{}/commands", s.app_id),
            &s.member.auth_header(),
            &json!([]),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn invoking_checks_options_and_tells_the_bot() {
    let s = setup().await;
    let mut rx = subscribe(&s.server).await;
    let interaction = play(&s, &mut rx).await;
    assert_eq!(interaction["type"], "application_command");
    assert_eq!(interaction["data"]["name"], "play");
    assert_eq!(interaction["data"]["options"][0]["value"], "daft punk");
    assert_eq!(interaction["user"]["id"], s.member.user.id.as_str());

    let id = command_id(&s, "play").await;
    let (status, _) = call(
        &s.server,
        authenticated_json_request(
            Method::POST,
            "/api/v1/interactions",
            &s.member.auth_header(),
            &json!({"type": "application_command", "channel_id": s.channel, "command_id": id, "data": {}}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_message_answer_shows_who_used_the_command() {
    let s = setup().await;
    let mut rx = subscribe(&s.server).await;
    let interaction = play(&s, &mut rx).await;

    let (status, _) = call(
        &s.server,
        anonymous(
            Method::POST,
            &callback_uri(&interaction, "callback"),
            &json!({"type": 4, "data": {
                "content": "Now playing: Around the World",
                "components": [{"type": 1, "components": [
                    {"type": 2, "style": 2, "label": "Skip", "custom_id": "skip"}
                ]}]
            }}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let success = next_event(&mut rx, "interaction.success").unwrap();
    assert_eq!(
        success.target_user_ids,
        Some(vec![s.member.user.id.clone()])
    );

    let messages = history(&s).await;
    let answer = messages
        .iter()
        .find(|m| m["content"] == "Now playing: Around the World")
        .unwrap();
    assert_eq!(answer["author_id"], s.bot.user.id.as_str());
    assert_eq!(answer["interaction"]["name"], "play");
    assert_eq!(answer["interaction"]["user_id"], s.member.user.id.as_str());
    assert_eq!(
        answer["components"][0]["components"][0]["custom_id"],
        "skip"
    );

    // One first answer only.
    let (status, _) = call(
        &s.server,
        anonymous(
            Method::POST,
            &callback_uri(&interaction, "callback"),
            &json!({"type": 4, "data": {"content": "again"}}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_deferred_answer_is_filled_in_later() {
    let s = setup().await;
    let mut rx = subscribe(&s.server).await;
    let interaction = play(&s, &mut rx).await;
    let (status, _) = call(
        &s.server,
        anonymous(
            Method::POST,
            &callback_uri(&interaction, "callback"),
            &json!({"type": 5}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let placeholder = history(&s)
        .await
        .into_iter()
        .find(|m| m["author_id"] == s.bot.user.id.as_str())
        .unwrap();
    assert_eq!(placeholder["flags"].as_i64().unwrap() & 128, 128);

    let (status, filled) = call(
        &s.server,
        anonymous(
            Method::PATCH,
            &callback_uri(&interaction, "original"),
            &json!({"content": "Found it"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{filled}");
    assert_eq!(filled["content"], "Found it");
    assert_eq!(filled["flags"].as_i64().unwrap() & 128, 0);
    assert!(filled["edited_at"].is_null());

    let (status, followup) = call(
        &s.server,
        anonymous(
            Method::POST,
            &callback_uri(&interaction, "followup"),
            &json!({"content": "Queued 3 more"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(followup["content"], "Queued 3 more");
}

#[tokio::test]
async fn an_ephemeral_answer_reaches_only_the_member() {
    let s = setup().await;
    let mut rx = subscribe(&s.server).await;
    let interaction = play(&s, &mut rx).await;
    let (status, _) = call(
        &s.server,
        anonymous(
            Method::POST,
            &callback_uri(&interaction, "callback"),
            &json!({"type": "message", "data": {"content": "You are not in voice", "flags": 64}}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let created = next_event(&mut rx, "message.create").unwrap();
    assert_eq!(
        created.target_user_ids,
        Some(vec![s.member.user.id.clone()])
    );
    assert_eq!(created.event["data"]["flags"], 64);
    assert!(history(&s)
        .await
        .iter()
        .all(|m| m["content"] != "You are not in voice"));

    let (status, edited) = call(
        &s.server,
        anonymous(
            Method::PATCH,
            &callback_uri(&interaction, "original"),
            &json!({"content": "Join a voice channel first"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(edited["content"], "Join a voice channel first");
}

#[tokio::test]
async fn a_button_click_lets_the_bot_update_its_message() {
    let s = setup().await;
    let mut rx = subscribe(&s.server).await;
    let interaction = play(&s, &mut rx).await;
    call(
        &s.server,
        anonymous(
            Method::POST,
            &callback_uri(&interaction, "callback"),
            &json!({"type": 4, "data": {
                "content": "Now playing",
                "components": [{"type": 1, "components": [
                    {"type": 2, "style": 1, "label": "Pause", "custom_id": "pause"},
                    {"type": 2, "style": 4, "label": "Stop", "custom_id": "stop", "disabled": true}
                ]}]
            }}),
        ),
    )
    .await;
    let message = history(&s)
        .await
        .into_iter()
        .find(|m| m["content"] == "Now playing")
        .unwrap();
    let message_id = message["id"].as_str().unwrap().to_string();

    let click = |custom_id: &str| {
        authenticated_json_request(
            Method::POST,
            "/api/v1/interactions",
            &s.member.auth_header(),
            &json!({"type": "message_component", "channel_id": s.channel,
                    "message_id": message_id, "data": {"custom_id": custom_id}}),
        )
    };
    let (status, _) = call(&s.server, click("stop")).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "disabled buttons can't be used"
    );
    let (status, _) = call(&s.server, click("nope")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = call(&s.server, click("pause")).await;
    assert_eq!(status, StatusCode::OK);
    let pressed = next_event(&mut rx, "interaction.create").unwrap().event["data"].clone();
    assert_eq!(pressed["type"], "message_component");
    assert_eq!(pressed["data"]["custom_id"], "pause");
    assert_eq!(pressed["message"]["id"], message_id.as_str());

    let (status, _) = call(
        &s.server,
        anonymous(
            Method::POST,
            &callback_uri(&pressed, "callback"),
            &json!({"type": 7, "data": {
                "content": "Paused",
                "components": [{"type": 1, "components": [
                    {"type": 2, "style": 3, "label": "Resume", "custom_id": "resume"}
                ]}]
            }}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let updated = history(&s)
        .await
        .into_iter()
        .find(|m| m["id"] == message_id.as_str())
        .unwrap();
    assert_eq!(updated["content"], "Paused");
    assert_eq!(
        updated["components"][0]["components"][0]["custom_id"],
        "resume"
    );
}

#[tokio::test]
async fn a_bot_sets_components_on_its_own_messages_only() {
    let s = setup().await;
    let (_, posted) = call(
        &s.server,
        authenticated_json_request(
            Method::POST,
            &format!("/api/v1/channels/{}/messages", s.channel),
            &s.bot.auth_header(),
            &json!({"content": "Now playing"}),
        ),
    )
    .await;
    let message_id = posted["data"]["id"].as_str().unwrap().to_string();
    let uri = format!(
        "/api/v1/channels/{}/messages/{message_id}/components",
        s.channel
    );
    let row = json!({"components": [{"type": 1, "components": [
        {"type": 2, "label": "Skip", "custom_id": "skip"}
    ]}]});
    let (status, _) = call(
        &s.server,
        authenticated_json_request(Method::PUT, &uri, &s.member.auth_header(), &row),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = call(
        &s.server,
        authenticated_json_request(Method::PUT, &uri, &s.bot.auth_header(), &row),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["data"]["components"][0]["components"][0]["style"],
        "secondary"
    );
}

#[tokio::test]
async fn an_expired_interaction_cannot_be_answered() {
    let s = setup().await;
    let mut rx = subscribe(&s.server).await;
    let interaction = play(&s, &mut rx).await;
    sqlx::query(&accordserver::db::q(
        "UPDATE interactions SET created_at = '2000-01-01T00:00:00+00:00' WHERE id = ?",
    ))
    .bind(interaction["id"].as_str().unwrap())
    .execute(s.server.pool())
    .await
    .unwrap();
    let (status, _) = call(
        &s.server,
        anonymous(
            Method::POST,
            &callback_uri(&interaction, "callback"),
            &json!({"type": 4, "data": {"content": "late"}}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let wrong = format!(
        "/api/v1/interactions/{}/{}/callback",
        interaction["id"].as_str().unwrap(),
        "x".repeat(64)
    );
    let (status, _) = call(
        &s.server,
        anonymous(Method::POST, &wrong, &json!({"type": 1})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
