//! A space's welcome screen: managers set it, members read it.
mod common;

use common::{authenticated_json_request, authenticated_request, TestServer};
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

#[tokio::test]
async fn managers_set_the_welcome_screen_members_read_it() {
    let server = TestServer::new().await;
    let owner = server.create_user_with_token("owner").await;
    let member = server.create_user_with_token("member").await;
    let space = server.create_space(&owner.user.id, "space").await;
    server.add_member(&space, &member.user.id).await;
    let general = server.create_channel(&space, "general").await;
    let uri = format!("/api/v1/spaces/{space}/welcome-screen");

    let (_, empty) = call(
        &server,
        authenticated_request(Method::GET, &uri, &member.auth_header()),
    )
    .await;
    assert_eq!(empty["data"]["enabled"], false);

    let body = json!({"enabled": true, "description": "Welcome!",
        "channels": [{"channel_id": general, "description": "Say hi", "emoji": "👋"}]});
    let (status, _) = call(
        &server,
        authenticated_json_request(Method::PATCH, &uri, &member.auth_header(), &body),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, saved) = call(
        &server,
        authenticated_json_request(Method::PATCH, &uri, &owner.auth_header(), &body),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");

    let (_, read) = call(
        &server,
        authenticated_request(Method::GET, &uri, &member.auth_header()),
    )
    .await;
    assert_eq!(read["data"]["description"], "Welcome!");
    assert_eq!(read["data"]["channels"][0]["emoji"], "👋");

    // Only what's given changes.
    call(
        &server,
        authenticated_json_request(
            Method::PATCH,
            &uri,
            &owner.auth_header(),
            &json!({"enabled": false}),
        ),
    )
    .await;
    let (_, read) = call(
        &server,
        authenticated_request(Method::GET, &uri, &member.auth_header()),
    )
    .await;
    assert_eq!(read["data"]["enabled"], false);
    assert_eq!(read["data"]["description"], "Welcome!");

    let other_space = server.create_space(&owner.user.id, "other").await;
    let elsewhere = server.create_channel(&other_space, "x").await;
    let (status, _) = call(
        &server,
        authenticated_json_request(
            Method::PATCH,
            &uri,
            &owner.auth_header(),
            &json!({"channels": [{"channel_id": elsewhere, "description": "Nope"}]}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
