mod common;

use common::{authenticated_json_request, authenticated_request, parse_body, TestServer};
use http::{Method, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

async fn call(
    server: &TestServer,
    method: Method,
    uri: &str,
    auth: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let req = match body {
        Some(body) => authenticated_json_request(method, uri, auth, &body),
        None => authenticated_request(method, uri, auth),
    };
    let response = server.router().oneshot(req).await.unwrap();
    let status = response.status();
    (status, parse_body(response).await)
}

struct Setup {
    server: TestServer,
    owner: String,
    member: String,
    member_id: String,
    channel_id: String,
}

async fn setup() -> Setup {
    let server = TestServer::new().await;
    let owner = server.create_user_with_token("pollowner").await;
    let member = server.create_user_with_token("pollmember").await;
    let space_id = server.create_space(&owner.user.id, "Polls").await;
    server.add_member(&space_id, &member.user.id).await;
    let channel_id = server.create_channel(&space_id, "general").await;
    Setup {
        owner: owner.auth_header(),
        member: member.auth_header(),
        member_id: member.user.id.clone(),
        channel_id,
        server,
    }
}

fn votes(poll: &Value) -> Vec<i64> {
    poll["answers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["votes"].as_i64().unwrap())
        .collect()
}

fn mine(poll: &Value) -> Vec<bool> {
    poll["answers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["me"].as_bool().unwrap())
        .collect()
}

async fn create(s: &Setup, body: Value) -> (StatusCode, Value) {
    call(
        &s.server,
        Method::POST,
        &format!("/api/v1/channels/{}/polls", s.channel_id),
        &s.owner,
        Some(body),
    )
    .await
}

#[tokio::test]
async fn creates_a_poll_message() {
    let s = setup().await;
    let (status, body) = create(&s, json!({"question": "Pizza or sushi?", "answers": [{"text": "Pizza"}, {"text": "Sushi", "emoji": "🍣"}]})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let poll = &body["data"]["poll"];
    assert_eq!(poll["question"], "Pizza or sushi?");
    assert_eq!(poll["allow_multiselect"], false);
    assert_eq!(poll["closed"], false);
    assert_eq!(poll["total_votes"], 0);
    assert_eq!(votes(poll), vec![0, 0]);
    assert_eq!(mine(poll), vec![false, false]);
    assert_eq!(poll["answers"][1]["emoji"], "🍣");
    assert_eq!(poll["answers"][0]["answer_id"], 1);
}

#[tokio::test]
async fn rejects_invalid_polls_and_direct_messages() {
    let s = setup().await;
    let answers: Vec<Value> = (0..11).map(|i| json!({"text": format!("a{i}")})).collect();
    let (status, _) = create(&s, json!({"question": "too many", "answers": answers})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = create(&s, json!({"question": "", "answers": [{"text": "a"}]})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let owner_id = s.server.create_user_with_token("dmpeer").await.user.id;
    let dm = s.server.create_dm(&s.member_id, &owner_id).await;
    let (status, _) = call(
        &s.server,
        Method::POST,
        &format!("/api/v1/channels/{dm}/polls"),
        &s.member,
        Some(json!({"question": "q", "answers": [{"text": "a"}]})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn single_choice_votes_replace_and_multiselect_votes_add() {
    let s = setup().await;
    let (_, body) = create(
        &s,
        json!({"question": "One?", "answers": [{"text": "A"}, {"text": "B"}]}),
    )
    .await;
    let message_id = body["data"]["id"].as_str().unwrap().to_string();
    let vote = |answer: i64| {
        format!(
            "/api/v1/channels/{}/polls/{message_id}/answers/{answer}/@me",
            s.channel_id
        )
    };

    let (status, body) = call(&s.server, Method::PUT, &vote(1), &s.member, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, body) = call(&s.server, Method::PUT, &vote(2), &s.member, None).await;
    assert_eq!(votes(&body["data"]["poll"]), vec![0, 1]);
    assert_eq!(mine(&body["data"]["poll"]), vec![false, true]);

    // The owner sees the count, not the member's choice as their own.
    let (_, list) = call(
        &s.server,
        Method::GET,
        &format!("/api/v1/channels/{}/messages", s.channel_id),
        &s.owner,
        None,
    )
    .await;
    let poll = &list["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == message_id.as_str())
        .unwrap()["poll"];
    assert_eq!(votes(poll), vec![0, 1]);
    assert_eq!(mine(poll), vec![false, false]);

    let (_, body) = call(&s.server, Method::DELETE, &vote(2), &s.member, None).await;
    assert_eq!(votes(&body["data"]["poll"]), vec![0, 0]);
    assert_eq!(body["data"]["poll"]["total_votes"], 0);

    let (status, _) = call(&s.server, Method::PUT, &vote(9), &s.member, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (_, body) = create(&s, json!({"question": "Many?", "answers": [{"text": "A"}, {"text": "B"}], "allow_multiselect": true})).await;
    let multi = body["data"]["id"].as_str().unwrap().to_string();
    let vote_multi = |answer: i64| {
        format!(
            "/api/v1/channels/{}/polls/{multi}/answers/{answer}/@me",
            s.channel_id
        )
    };
    call(&s.server, Method::PUT, &vote_multi(1), &s.member, None).await;
    let (_, body) = call(&s.server, Method::PUT, &vote_multi(2), &s.member, None).await;
    assert_eq!(votes(&body["data"]["poll"]), vec![1, 1]);
    assert_eq!(body["data"]["poll"]["total_votes"], 2);
}

#[tokio::test]
async fn only_the_author_or_a_moderator_ends_a_poll_and_then_voting_stops() {
    let s = setup().await;
    let (_, body) = create(&s, json!({"question": "End?", "answers": [{"text": "A"}]})).await;
    let message_id = body["data"]["id"].as_str().unwrap().to_string();
    let expire = format!(
        "/api/v1/channels/{}/polls/{message_id}/expire",
        s.channel_id
    );

    let (status, _) = call(&s.server, Method::POST, &expire, &s.member, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = call(&s.server, Method::POST, &expire, &s.owner, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["poll"]["closed"], true);

    let (status, _) = call(
        &s.server,
        Method::PUT,
        &format!(
            "/api/v1/channels/{}/polls/{message_id}/answers/1/@me",
            s.channel_id
        ),
        &s.member,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn deleting_the_message_removes_its_poll() {
    let s = setup().await;
    let (_, body) = create(&s, json!({"question": "Gone?", "answers": [{"text": "A"}]})).await;
    let message_id = body["data"]["id"].as_str().unwrap().to_string();
    call(
        &s.server,
        Method::PUT,
        &format!(
            "/api/v1/channels/{}/polls/{message_id}/answers/1/@me",
            s.channel_id
        ),
        &s.member,
        None,
    )
    .await;
    let (status, _) = call(
        &s.server,
        Method::DELETE,
        &format!("/api/v1/channels/{}/messages/{message_id}", s.channel_id),
        &s.owner,
        None,
    )
    .await;
    assert!(status.is_success(), "{status}");
    for table in ["polls", "poll_answers", "poll_votes"] {
        let left: i64 = sqlx::query_scalar(&format!(
            "SELECT COUNT(*) FROM {table} WHERE message_id = ?"
        ))
        .bind(&message_id)
        .fetch_one(s.server.pool())
        .await
        .unwrap();
        assert_eq!(left, 0, "{table}");
    }
}
