//! Listing a channel's pinned messages.
mod common;

use common::{authenticated_json_request, authenticated_request, parse_body, TestServer};
use http::{Method, StatusCode};
use serde_json::json;
use tower::ServiceExt;

#[tokio::test]
async fn pinned_messages_are_listed() {
    let server = TestServer::new().await;
    let owner = server.create_user_with_token("owner").await;
    let space = server.create_space(&owner.user.id, "space").await;
    let channel = server.create_channel(&space, "general").await;
    let response = server
        .router()
        .oneshot(authenticated_json_request(
            Method::POST,
            &format!("/api/v1/channels/{channel}/messages"),
            &owner.auth_header(),
            &json!({ "content": "keep me" }),
        ))
        .await
        .unwrap();
    let message = parse_body(response).await["data"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let response = server
        .router()
        .oneshot(authenticated_request(
            Method::PUT,
            &format!("/api/v1/channels/{channel}/pins/{message}"),
            &owner.auth_header(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = server
        .router()
        .oneshot(authenticated_request(
            Method::GET,
            &format!("/api/v1/channels/{channel}/pins"),
            &owner.auth_header(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = parse_body(response).await;
    assert_eq!(body["data"][0]["id"], message);
}
