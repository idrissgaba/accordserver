//! Stickers: managed with `manage_emojis`, sent one per message, usable in
//! another space the sender belongs to.
mod common;

use base64::Engine;
use common::{authenticated_json_request, authenticated_request, TestServer, TestUser};
use http::{Method, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

fn png() -> String {
    let bytes: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
        0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08, 0xD7, 0x63, 0xF8,
        0xCF, 0xC0, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01, 0xE2, 0x21, 0xBC, 0x33, 0x00, 0x00, 0x00,
        0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];
    format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

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

async fn upload(
    server: &TestServer,
    space: &str,
    who: &TestUser,
    name: &str,
) -> (StatusCode, Value) {
    call(
        server,
        authenticated_json_request(
            Method::POST,
            &format!("/api/v1/spaces/{space}/stickers"),
            &who.auth_header(),
            &json!({"name": name, "description": "A party", "tags": "🎉", "image": png()}),
        ),
    )
    .await
}

#[tokio::test]
async fn managers_upload_rename_and_delete_stickers() {
    let server = TestServer::new().await;
    let owner = server.create_user_with_token("owner").await;
    let member = server.create_user_with_token("member").await;
    let space = server.create_space(&owner.user.id, "space").await;
    server.add_member(&space, &member.user.id).await;

    let (status, _) = upload(&server, &space, &member, "Party").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, created) = upload(&server, &space, &owner, "Party").await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let sticker = created["data"].clone();
    assert_eq!(sticker["format"], "png");
    assert!(sticker["image"]
        .as_str()
        .unwrap()
        .starts_with("/cdn/emojis/"));

    let (_, listed) = call(
        &server,
        authenticated_request(
            Method::GET,
            &format!("/api/v1/spaces/{space}/stickers"),
            &member.auth_header(),
        ),
    )
    .await;
    assert_eq!(listed["data"].as_array().unwrap().len(), 1);

    let uri = format!(
        "/api/v1/spaces/{space}/stickers/{}",
        sticker["id"].as_str().unwrap()
    );
    let (_, renamed) = call(
        &server,
        authenticated_json_request(
            Method::PATCH,
            &uri,
            &owner.auth_header(),
            &json!({"name": "Fiesta"}),
        ),
    )
    .await;
    assert_eq!(renamed["data"]["name"], "Fiesta");
    assert_eq!(renamed["data"]["description"], "A party");

    let (status, _) = call(
        &server,
        authenticated_request(Method::DELETE, &uri, &owner.auth_header()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, listed) = call(
        &server,
        authenticated_request(
            Method::GET,
            &format!("/api/v1/spaces/{space}/stickers"),
            &member.auth_header(),
        ),
    )
    .await;
    assert!(listed["data"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn a_sticker_is_sent_as_a_message() {
    let server = TestServer::new().await;
    let owner = server.create_user_with_token("owner").await;
    let space = server.create_space(&owner.user.id, "space").await;
    let channel = server.create_channel(&space, "general").await;
    let (_, created) = upload(&server, &space, &owner, "Party").await;
    let sticker_id = created["data"]["id"].as_str().unwrap().to_string();

    let (status, sent) = call(
        &server,
        authenticated_json_request(
            Method::POST,
            &format!("/api/v1/channels/{channel}/stickers/{sticker_id}"),
            &owner.auth_header(),
            &json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{sent}");
    assert_eq!(sent["data"]["stickers"][0]["id"], sticker_id.as_str());
    assert_eq!(sent["data"]["stickers"][0]["name"], "Party");

    let (_, history) = call(
        &server,
        authenticated_request(
            Method::GET,
            &format!("/api/v1/channels/{channel}/messages"),
            &owner.auth_header(),
        ),
    )
    .await;
    assert_eq!(history["data"][0]["stickers"][0]["format"], "png");
}

#[tokio::test]
async fn another_spaces_sticker_needs_membership_there() {
    let server = TestServer::new().await;
    let owner = server.create_user_with_token("owner").await;
    let friend = server.create_user_with_token("friend").await;
    let home = server.create_space(&owner.user.id, "home").await;
    let other = server.create_space(&owner.user.id, "other").await;
    server.add_member(&home, &friend.user.id).await;
    let channel = server.create_channel(&home, "general").await;
    let (_, created) = upload(&server, &other, &owner, "Elsewhere").await;
    let uri = format!(
        "/api/v1/channels/{channel}/stickers/{}",
        created["data"]["id"].as_str().unwrap()
    );
    // The owner belongs to both spaces.
    let (status, _) = call(
        &server,
        authenticated_json_request(Method::POST, &uri, &owner.auth_header(), &json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // The friend doesn't belong to the sticker's space.
    let (status, _) = call(
        &server,
        authenticated_json_request(Method::POST, &uri, &friend.auth_header(), &json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
