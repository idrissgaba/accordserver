//! Super reactions are counted apart from plain reactions and need the
//! `add_reactions` right in the channel.
mod common;

use common::{authenticated_json_request, authenticated_request, parse_body, TestServer};
use http::{Method, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

async fn call(
    server: &TestServer,
    request: http::Request<axum::body::Body>,
) -> (StatusCode, Value) {
    let response = server.router().oneshot(request).await.unwrap();
    let status = response.status();
    (status, parse_body(response).await)
}

#[tokio::test]
async fn super_reactions_are_counted_apart() {
    let server = TestServer::new().await;
    let owner = server.create_user_with_token("owner").await;
    let member = server.create_user_with_token("member").await;
    let space = server.create_space(&owner.user.id, "space").await;
    server.add_member(&space, &member.user.id).await;
    let channel = server.create_channel(&space, "general").await;

    let (status, body) = call(
        &server,
        authenticated_json_request(
            Method::POST,
            &format!("/api/v1/channels/{channel}/messages"),
            &owner.auth_header(),
            &json!({ "content": "hello" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let message = body["data"]["id"].as_str().unwrap().to_string();
    let uri = format!("/api/v1/channels/{channel}/messages/{message}/super-reactions/%F0%9F%94%A5");

    for user in [&owner, &member] {
        let (status, _) = call(
            &server,
            authenticated_request(Method::PUT, &uri, &user.auth_header()),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
    // Adding twice changes nothing.
    call(
        &server,
        authenticated_request(Method::PUT, &uri, &member.auth_header()),
    )
    .await;

    let (_, body) = call(
        &server,
        authenticated_request(
            Method::GET,
            &format!("/api/v1/channels/{channel}/messages/{message}"),
            &member.auth_header(),
        ),
    )
    .await;
    let message_json = &body["data"];
    assert_eq!(
        message_json["super_reactions"],
        json!([{ "emoji": "🔥", "count": 2, "me": true }])
    );
    // Plain reactions are untouched.
    assert!(message_json["reactions"]
        .as_array()
        .is_none_or(|r| r.is_empty()));

    let (status, _) = call(
        &server,
        authenticated_request(Method::DELETE, &uri, &member.auth_header()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = call(
        &server,
        authenticated_request(
            Method::GET,
            &format!("/api/v1/channels/{channel}/messages/{message}"),
            &member.auth_header(),
        ),
    )
    .await;
    assert_eq!(
        body["data"]["super_reactions"],
        json!([{ "emoji": "🔥", "count": 1, "me": false }])
    );
}

#[tokio::test]
async fn outsiders_cannot_super_react() {
    let server = TestServer::new().await;
    let owner = server.create_user_with_token("owner").await;
    let outsider = server.create_user_with_token("outsider").await;
    let space = server.create_space(&owner.user.id, "space").await;
    let channel = server.create_channel(&space, "general").await;
    let (_, body) = call(
        &server,
        authenticated_json_request(
            Method::POST,
            &format!("/api/v1/channels/{channel}/messages"),
            &owner.auth_header(),
            &json!({ "content": "hello" }),
        ),
    )
    .await;
    let message = body["data"]["id"].as_str().unwrap().to_string();
    let (status, _) = call(
        &server,
        authenticated_request(
            Method::PUT,
            &format!("/api/v1/channels/{channel}/messages/{message}/super-reactions/x"),
            &outsider.auth_header(),
        ),
    )
    .await;
    assert!(status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND);
}
