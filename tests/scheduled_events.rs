//! Scheduled events: planning behind `manage_events`, interest, and the
//! scheduler starting and ending events on time.
mod common;

use common::{authenticated_json_request, authenticated_request, TestServer, TestUser};
use http::{Method, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

async fn call(
    server: &TestServer,
    request: http::Request<axum::body::Body>,
) -> (StatusCode, Value) {
    let response = server.router().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

struct Setup {
    server: TestServer,
    owner: TestUser,
    member: TestUser,
    space: String,
    voice: String,
}

async fn setup() -> Setup {
    let server = TestServer::new().await;
    let owner = server.create_user_with_token("owner").await;
    let member = server.create_user_with_token("member").await;
    let space = server.create_space(&owner.user.id, "space").await;
    server.add_member(&space, &member.user.id).await;
    let voice = server.create_voice_channel(&space, "Lounge").await;
    Setup {
        server,
        owner,
        member,
        space,
        voice,
    }
}

fn in_hours(hours: i64) -> String {
    (chrono::Utc::now() + chrono::Duration::hours(hours)).to_rfc3339()
}

async fn create(s: &Setup, body: Value) -> (StatusCode, Value) {
    call(
        &s.server,
        authenticated_json_request(
            Method::POST,
            &format!("/api/v1/spaces/{}/events", s.space),
            &s.owner.auth_header(),
            &body,
        ),
    )
    .await
}

async fn movie_night(s: &Setup) -> Value {
    let (status, body) = create(
        s,
        json!({"name": "Movie night", "entity_type": "voice", "channel_id": s.voice,
               "start_time": in_hours(2), "description": "Popcorn"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["data"].clone()
}

#[tokio::test]
async fn managers_plan_events_members_see_them() {
    let s = setup().await;
    let event = movie_night(&s).await;
    assert_eq!(event["status"], "scheduled");
    assert_eq!(event["user_count"], 1, "the creator is interested");
    assert_eq!(event["interested"], true);

    let (status, _) = call(
        &s.server,
        authenticated_json_request(
            Method::POST,
            &format!("/api/v1/spaces/{}/events", s.space),
            &s.member.auth_header(),
            &json!({"name": "x", "entity_type": "voice", "channel_id": s.voice, "start_time": in_hours(1)}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, listed) = call(
        &s.server,
        authenticated_request(
            Method::GET,
            &format!("/api/v1/spaces/{}/events", s.space),
            &s.member.auth_header(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["data"][0]["name"], "Movie night");
    assert_eq!(listed["data"][0]["interested"], false);
}

#[tokio::test]
async fn bad_events_are_refused() {
    let s = setup().await;
    let text = s.server.create_channel(&s.space, "general").await;
    for body in [
        json!({"name": "", "entity_type": "voice", "channel_id": s.voice, "start_time": in_hours(1)}),
        json!({"name": "a", "entity_type": "voice", "channel_id": text, "start_time": in_hours(1)}),
        json!({"name": "a", "entity_type": "voice", "channel_id": s.voice, "start_time": in_hours(-2)}),
        json!({"name": "a", "entity_type": "external", "location": "Park", "start_time": in_hours(1)}),
        json!({"name": "a", "entity_type": "external", "location": "Park", "start_time": in_hours(2), "end_time": in_hours(1)}),
    ] {
        let (status, body) = create(&s, body.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    }
    let (status, body) = create(
        &s,
        json!({"name": "Picnic", "entity_type": "external", "location": "Park",
               "start_time": in_hours(1), "end_time": in_hours(3)}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["location"], "Park");
}

#[tokio::test]
async fn members_mark_and_unmark_interest() {
    let s = setup().await;
    let event = movie_night(&s).await;
    let uri = format!(
        "/api/v1/spaces/{}/events/{}/users/@me",
        s.space,
        event["id"].as_str().unwrap()
    );
    let mut rx = s
        .server
        .state
        .gateway_tx
        .read()
        .await
        .as_ref()
        .unwrap()
        .subscribe();
    let (_, added) = call(
        &s.server,
        authenticated_request(Method::PUT, &uri, &s.member.auth_header()),
    )
    .await;
    assert_eq!(added["data"]["user_count"], 2);
    assert_eq!(added["data"]["interested"], true);
    let event_type = rx.try_recv().unwrap().event["type"].clone();
    assert_eq!(event_type, "scheduled_event.user_add");

    let (_, users) = call(
        &s.server,
        authenticated_request(
            Method::GET,
            &format!(
                "/api/v1/spaces/{}/events/{}/users",
                s.space,
                event["id"].as_str().unwrap()
            ),
            &s.member.auth_header(),
        ),
    )
    .await;
    assert_eq!(users["data"].as_array().unwrap().len(), 2);

    let (_, removed) = call(
        &s.server,
        authenticated_request(Method::DELETE, &uri, &s.member.auth_header()),
    )
    .await;
    assert_eq!(removed["data"]["user_count"], 1);
    assert_eq!(removed["data"]["interested"], false);
}

#[tokio::test]
async fn managers_edit_start_end_and_cancel() {
    let s = setup().await;
    let event = movie_night(&s).await;
    let uri = format!(
        "/api/v1/spaces/{}/events/{}",
        s.space,
        event["id"].as_str().unwrap()
    );
    let patch = |body: Value| {
        authenticated_json_request(Method::PATCH, &uri, &s.owner.auth_header(), &body)
    };

    let (status, edited) = call(&s.server, patch(json!({"name": "Film night"}))).await;
    assert_eq!(status, StatusCode::OK, "{edited}");
    assert_eq!(edited["data"]["name"], "Film night");
    assert_eq!(edited["data"]["description"], "Popcorn");

    let (status, _) = call(&s.server, patch(json!({"status": "completed"}))).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "can't end what hasn't started"
    );
    let (_, started) = call(&s.server, patch(json!({"status": "active"}))).await;
    assert_eq!(started["data"]["status"], "active");
    let (status, _) = call(&s.server, patch(json!({"start_time": in_hours(5)}))).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "no rescheduling once under way"
    );
    let (_, ended) = call(&s.server, patch(json!({"status": "completed"}))).await;
    assert_eq!(ended["data"]["status"], "completed");

    // Finished events leave the list.
    let (_, listed) = call(
        &s.server,
        authenticated_request(
            Method::GET,
            &format!("/api/v1/spaces/{}/events", s.space),
            &s.member.auth_header(),
        ),
    )
    .await;
    assert!(listed["data"].as_array().unwrap().is_empty());

    let other = movie_night(&s).await;
    let other_uri = format!(
        "/api/v1/spaces/{}/events/{}",
        s.space,
        other["id"].as_str().unwrap()
    );
    let (_, canceled) = call(
        &s.server,
        authenticated_json_request(
            Method::PATCH,
            &other_uri,
            &s.owner.auth_header(),
            &json!({"status": "canceled"}),
        ),
    )
    .await;
    assert_eq!(canceled["data"]["status"], "canceled");
    let (status, _) = call(
        &s.server,
        authenticated_request(Method::DELETE, &other_uri, &s.owner.auth_header()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(
        &s.server,
        authenticated_request(Method::GET, &other_uri, &s.owner.auth_header()),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_scheduler_starts_and_ends_events() {
    let s = setup().await;
    let event = movie_night(&s).await;
    let id = event["id"].as_str().unwrap();
    // Pretend the start time came and went.
    sqlx::query(&accordserver::db::q(
        "UPDATE scheduled_events SET start_time = ?, end_time = ? WHERE id = ?",
    ))
    .bind(
        (chrono::Utc::now() - chrono::Duration::minutes(1))
            .format("%Y-%m-%dT%H:%M:%S+00:00")
            .to_string(),
    )
    .bind(
        (chrono::Utc::now() + chrono::Duration::hours(1))
            .format("%Y-%m-%dT%H:%M:%S+00:00")
            .to_string(),
    )
    .bind(id)
    .execute(s.server.pool())
    .await
    .unwrap();
    let mut rx = s
        .server
        .state
        .gateway_tx
        .read()
        .await
        .as_ref()
        .unwrap()
        .subscribe();
    accordserver::routes::scheduled_events::tick(&s.server.state).await;
    let update = rx.try_recv().unwrap();
    assert_eq!(update.event["type"], "scheduled_event.update");
    assert_eq!(update.event["data"]["status"], "active");
    assert_eq!(update.intent, "scheduled_events");

    sqlx::query(&accordserver::db::q(
        "UPDATE scheduled_events SET end_time = ? WHERE id = ?",
    ))
    .bind(
        (chrono::Utc::now() - chrono::Duration::seconds(1))
            .format("%Y-%m-%dT%H:%M:%S+00:00")
            .to_string(),
    )
    .bind(id)
    .execute(s.server.pool())
    .await
    .unwrap();
    accordserver::routes::scheduled_events::tick(&s.server.state).await;
    let done = accordserver::db::scheduled_events::get(s.server.pool(), id)
        .await
        .unwrap();
    assert_eq!(done.status, "completed");
}
