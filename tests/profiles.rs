//! Profile extras (pronouns, theme colours) and private notes.
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
async fn a_user_sets_pronouns_and_theme_others_see_them() {
    let server = TestServer::new().await;
    let ada = server.create_user_with_token("ada").await;
    let bob = server.create_user_with_token("bob").await;
    let (status, body) = call(
        &server,
        authenticated_json_request(
            Method::PATCH,
            "/api/v1/users/@me/profile",
            &ada.auth_header(),
            &json!({"pronouns": "she/her", "theme_primary": 5793266, "theme_accent": 15548997}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, mine) = call(
        &server,
        authenticated_request(Method::GET, "/api/v1/users/@me/profile", &ada.auth_header()),
    )
    .await;
    assert_eq!(mine["data"]["pronouns"], "she/her");
    let (_, seen) = call(
        &server,
        authenticated_request(
            Method::GET,
            &format!("/api/v1/users/{}/profile", ada.user.id),
            &bob.auth_header(),
        ),
    )
    .await;
    assert_eq!(seen["data"]["theme_primary"], 5793266);
    assert!(seen["data"].get("note").is_none());

    let (status, _) = call(
        &server,
        authenticated_json_request(
            Method::PATCH,
            "/api/v1/users/@me/profile",
            &ada.auth_header(),
            &json!({"theme_accent": -1}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn notes_are_private_to_their_author() {
    let server = TestServer::new().await;
    let ada = server.create_user_with_token("ada").await;
    let bob = server.create_user_with_token("bob").await;
    let uri = format!("/api/v1/users/@me/notes/{}", bob.user.id);
    let (status, _) = call(
        &server,
        authenticated_json_request(
            Method::PUT,
            &uri,
            &ada.auth_header(),
            &json!({"note": "Met at the meetup"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, profile) = call(
        &server,
        authenticated_request(
            Method::GET,
            &format!("/api/v1/users/{}/profile", bob.user.id),
            &ada.auth_header(),
        ),
    )
    .await;
    assert_eq!(profile["data"]["note"], "Met at the meetup");
    // Bob sees his own profile without Ada's note.
    let (_, own) = call(
        &server,
        authenticated_request(
            Method::GET,
            &format!("/api/v1/users/{}/profile", bob.user.id),
            &bob.auth_header(),
        ),
    )
    .await;
    assert!(own["data"].get("note").is_none());
    let (_, all) = call(
        &server,
        authenticated_request(Method::GET, "/api/v1/users/@me/notes", &ada.auth_header()),
    )
    .await;
    assert_eq!(all["data"][bob.user.id.as_str()], "Met at the meetup");

    call(
        &server,
        authenticated_json_request(Method::PUT, &uri, &ada.auth_header(), &json!({"note": ""})),
    )
    .await;
    let (_, all) = call(
        &server,
        authenticated_request(Method::GET, "/api/v1/users/@me/notes", &ada.auth_header()),
    )
    .await;
    assert!(all["data"].as_object().unwrap().is_empty());

    let (status, _) = call(
        &server,
        authenticated_json_request(
            Method::PUT,
            &format!("/api/v1/users/@me/notes/{}", ada.user.id),
            &ada.auth_header(),
            &json!({"note": "me"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
