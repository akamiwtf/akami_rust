//! End-to-end REST tests: spin up the real axum router on an ephemeral port
//! against a throwaway SQLite DB and drive it over HTTP with reqwest.

use axum::Router;
use tower_http::services::ServeDir;

use crate::{db, routes, state::AppState};

async fn spawn_server() -> String {
    let dir = std::env::temp_dir().join(format!("akami_it_{}.db", crate::db::new_id()));
    let url = format!("sqlite:{}", dir.display());
    let pool = db::connect(&url).await.unwrap();

    let cfg = crate::config::Config {
        port: 0,
        jwt_secret: "test-secret".into(),
        dm_encryption_key: "test-dm-key".into(),
        database_url: url,
    };
    let st = AppState::new(pool, cfg);

    let app = Router::new()
        .merge(routes::api_router())
        .nest_service("/uploads", ServeDir::new("uploads"))
        .with_state(st);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

#[tokio::test]
async fn full_rest_flow() {
    let base = spawn_server().await;
    let c = client();

    // --- register ---
    let r = c
        .post(format!("{base}/api/auth/register"))
        .json(&serde_json::json!({ "username": "alice", "email": "a@x.com", "password": "pw123" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201);
    let body: serde_json::Value = r.json().await.unwrap();
    let token_a = body["token"].as_str().unwrap().to_string();
    assert_eq!(body["user"]["username"], "alice");
    assert!(body["user"]["displayName"].is_null());

    // duplicate registration rejected
    let r = c
        .post(format!("{base}/api/auth/register"))
        .json(&serde_json::json!({ "username": "alice", "email": "a@x.com", "password": "pw" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
    assert_eq!(
        r.json::<serde_json::Value>().await.unwrap()["error"],
        "Username or email already exists"
    );

    // --- login ---
    let r = c
        .post(format!("{base}/api/auth/login"))
        .json(&serde_json::json!({ "email": "a@x.com", "password": "pw123" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let login_token = r.json::<serde_json::Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();

    // wrong password
    let r = c
        .post(format!("{base}/api/auth/login"))
        .json(&serde_json::json!({ "email": "a@x.com", "password": "nope" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);

    // --- me ---
    let r = c
        .get(format!("{base}/api/auth/me"))
        .bearer_auth(&login_token)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(
        r.json::<serde_json::Value>().await.unwrap()["user"]["email"],
        "a@x.com"
    );

    // no token -> 401
    let r = c.get(format!("{base}/api/auth/me")).send().await.unwrap();
    assert_eq!(r.status(), 401);

    // --- create server ---
    let r = c
        .post(format!("{base}/api/servers"))
        .bearer_auth(&token_a)
        .json(&serde_json::json!({ "name": "My Server" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201);
    let server: serde_json::Value = r.json().await.unwrap();
    let server_id = server["id"].as_str().unwrap().to_string();
    let invite = server["inviteCode"].as_str().unwrap().to_string();
    assert_eq!(server["channels"][0]["name"], "general");
    assert_eq!(server["channels"][0]["type"], "TEXT");
    let channel_id = server["channels"][0]["id"].as_str().unwrap().to_string();

    // --- list servers ---
    let r = c
        .get(format!("{base}/api/servers"))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap();
    let servers: serde_json::Value = r.json().await.unwrap();
    assert_eq!(servers.as_array().unwrap().len(), 1);
    assert_eq!(servers[0]["channels"][0]["name"], "general");

    // --- create channel ---
    let r = c
        .post(format!("{base}/api/servers/{server_id}/channels"))
        .bearer_auth(&token_a)
        .json(&serde_json::json!({ "name": "  voice-chat  ", "type": "VOICE" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201);
    let ch: serde_json::Value = r.json().await.unwrap();
    assert_eq!(ch["name"], "voice-chat"); // trimmed
    assert_eq!(ch["type"], "VOICE");

    // channels list now has 2
    let r = c
        .get(format!("{base}/api/servers/{server_id}/channels"))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap();
    assert_eq!(r.json::<serde_json::Value>().await.unwrap().as_array().unwrap().len(), 2);

    // messages empty
    let r = c
        .get(format!("{base}/api/servers/channels/{channel_id}/messages"))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.json::<serde_json::Value>().await.unwrap().as_array().unwrap().len(), 0);

    // --- second user joins via invite ---
    let r = c
        .post(format!("{base}/api/auth/register"))
        .json(&serde_json::json!({ "username": "bob", "email": "b@x.com", "password": "pw" }))
        .send()
        .await
        .unwrap();
    let token_b = r.json::<serde_json::Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();

    let r = c
        .post(format!("{base}/api/servers/join"))
        .bearer_auth(&token_b)
        .json(&serde_json::json!({ "inviteCode": invite }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    // members now 2
    let r = c
        .get(format!("{base}/api/servers/{server_id}/members"))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap();
    let members: serde_json::Value = r.json().await.unwrap();
    assert_eq!(members.as_array().unwrap().len(), 2);
    assert!(members
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["role"] == "ADMIN"));

    // --- friends ---
    let r = c
        .post(format!("{base}/api/friends/request"))
        .bearer_auth(&token_a)
        .json(&serde_json::json!({ "username": "bob" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let friendship: serde_json::Value = r.json().await.unwrap();
    let fid = friendship["id"].as_str().unwrap().to_string();
    assert_eq!(friendship["status"], "PENDING");
    assert_eq!(friendship["sender"]["username"], "alice");
    assert_eq!(friendship["receiver"]["username"], "bob");

    // duplicate request rejected in Russian
    let r = c
        .post(format!("{base}/api/friends/request"))
        .bearer_auth(&token_a)
        .json(&serde_json::json!({ "username": "bob" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
    assert_eq!(
        r.json::<serde_json::Value>().await.unwrap()["error"],
        "Заявка уже отправлена"
    );

    // bob accepts
    let r = c
        .post(format!("{base}/api/friends/accept/{fid}"))
        .bearer_auth(&token_b)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.json::<serde_json::Value>().await.unwrap()["status"], "ACCEPTED");

    // list friends for alice
    let r = c
        .get(format!("{base}/api/friends"))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap();
    let friends: serde_json::Value = r.json().await.unwrap();
    assert_eq!(friends.as_array().unwrap().len(), 1);
    assert_eq!(friends[0]["status"], "ACCEPTED");

    // --- profile update: set displayName, clear via null semantics ---
    let r = c
        .put(format!("{base}/api/users/profile"))
        .bearer_auth(&token_a)
        .json(&serde_json::json!({ "displayName": "Alice A", "bio": "hello" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let prof: serde_json::Value = r.json().await.unwrap();
    assert_eq!(prof["displayName"], "Alice A");
    assert_eq!(prof["bio"], "hello");

    // --- profile colour: survives the round trip, and is visible to others ---
    let r = c
        .put(format!("{base}/api/users/profile"))
        .bearer_auth(&token_a)
        .json(&serde_json::json!({ "profileColor": "#ff8800" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(
        r.json::<serde_json::Value>().await.unwrap()["profileColor"],
        "#ff8800"
    );

    // A re-read of the profile still has it (i.e. it was persisted, not echoed).
    // /auth/me is what the client caches as "me", so it has to be the complete
    // shape — a missing field there silently wipes it on the next save.
    let r = c
        .get(format!("{base}/api/auth/me"))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap();
    let me: serde_json::Value = r.json().await.unwrap();
    assert_eq!(me["user"]["profileColor"], "#ff8800");
    assert_eq!(me["user"]["displayName"], "Alice A");
    for field in ["username", "email", "bio", "pronouns", "badges", "customStatus", "socials"] {
        assert!(!me["user"][field].is_null(), "/auth/me is missing {field}");
    }

    // an unrelated profile save must not wipe the colour
    let r = c
        .put(format!("{base}/api/users/profile"))
        .bearer_auth(&token_a)
        .json(&serde_json::json!({ "bio": "still orange" }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.json::<serde_json::Value>().await.unwrap()["profileColor"],
        "#ff8800"
    );

    // and an empty string clears it back to "follow the viewer's theme"
    let r = c
        .put(format!("{base}/api/users/profile"))
        .bearer_auth(&token_a)
        .json(&serde_json::json!({ "profileColor": "" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.json::<serde_json::Value>().await.unwrap()["profileColor"], "");

    // put it back for the checks further down
    c.put(format!("{base}/api/users/profile"))
        .bearer_auth(&token_a)
        .json(&serde_json::json!({ "profileColor": "#ff8800" }))
        .send()
        .await
        .unwrap();

    // clear displayName with explicit null
    let r = c
        .put(format!("{base}/api/users/profile"))
        .bearer_auth(&token_a)
        .json(&serde_json::json!({ "displayName": null }))
        .send()
        .await
        .unwrap();
    assert!(r.json::<serde_json::Value>().await.unwrap()["displayName"].is_null());

    // --- search & single user & badges ---
    let r = c
        .get(format!("{base}/api/users/search?q=bo"))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap();
    let found: serde_json::Value = r.json().await.unwrap();
    assert_eq!(found[0]["username"], "bob");

    let bob_id = found[0]["id"].as_str().unwrap().to_string();
    let r = c
        .get(format!("{base}/api/users/{bob_id}/badges"))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap();
    let badges: serde_json::Value = r.json().await.unwrap();
    // bob is a brand-new non-bot user: expect the "newcomer" badge.
    assert!(badges
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b["id"] == "newcomer"));

    // mutuals (bob shares the server with alice)
    let r = c
        .get(format!("{base}/api/users/{bob_id}/mutuals"))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap();
    let mutuals: serde_json::Value = r.json().await.unwrap();
    assert_eq!(mutuals["mutualServers"].as_array().unwrap().len(), 1);

    // --- upload ---
    let data = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b"hello file");
    let r = c
        .post(format!("{base}/api/upload"))
        .bearer_auth(&token_a)
        .json(&serde_json::json!({ "filename": "logo.png", "fileData": data }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let up: serde_json::Value = r.json().await.unwrap();
    let url = up["url"].as_str().unwrap();
    assert!(url.contains("/uploads/logo_"));
    assert!(url.ends_with(".png"));

    // --- DM history (empty, but exercises decrypt path) ---
    let r = c
        .get(format!("{base}/api/users/dms/{bob_id}"))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.json::<serde_json::Value>().await.unwrap().as_array().unwrap().len(), 0);
}
