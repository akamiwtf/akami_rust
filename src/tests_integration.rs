//! End-to-end REST tests: spin up the real axum router on an ephemeral port
//! against a throwaway SQLite DB and drive it over HTTP with reqwest.

use axum::Router;

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
        .nest("/uploads", routes::uploads_router())
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

    // --- server settings: name, picture and banner, owner only ---
    let r = c
        .put(format!("{base}/api/servers/{server_id}"))
        .bearer_auth(&token_a)
        .json(&serde_json::json!({
            "name": "  Переименованный  ",
            "imageUrl": "http://x/icon.png",
            "bannerUrl": "http://x/banner.png",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let updated: serde_json::Value = r.json().await.unwrap();
    // Trimmed, and both images stored.
    assert_eq!(updated["name"], "Переименованный");
    assert_eq!(updated["imageUrl"], "http://x/icon.png");
    assert_eq!(updated["bannerUrl"], "http://x/banner.png");

    // A member who is not the owner cannot.
    let r = c
        .put(format!("{base}/api/servers/{server_id}"))
        .bearer_auth(&token_b)
        .json(&serde_json::json!({ "name": "чужое" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);

    // Clearing the banner: an explicit null, while a missing key keeps what is there.
    let r = c
        .put(format!("{base}/api/servers/{server_id}"))
        .bearer_auth(&token_a)
        .json(&serde_json::json!({ "bannerUrl": null }))
        .send()
        .await
        .unwrap();
    let cleared: serde_json::Value = r.json().await.unwrap();
    assert!(cleared["bannerUrl"].is_null());
    assert_eq!(cleared["imageUrl"], "http://x/icon.png", "untouched key must stay");
    assert_eq!(cleared["name"], "Переименованный");

    // --- invites: one-off, timed and permanent links ---
    // Permanent: no limits at all.
    let r = c
        .post(format!("{base}/api/servers/{server_id}/invites"))
        .bearer_auth(&token_a)
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201);
    let permanent: serde_json::Value = r.json().await.unwrap();
    assert!(permanent["code"].as_str().unwrap().len() >= 8);
    assert!(permanent["maxUses"].is_null());
    assert!(permanent.get("expiresAt").is_none(), "no expiry key when endless");
    assert_eq!(permanent["active"], true);
    assert_eq!(permanent["creator"]["username"], "alice");

    // One-off.
    let r = c
        .post(format!("{base}/api/servers/{server_id}/invites"))
        .bearer_auth(&token_a)
        .json(&serde_json::json!({ "maxUses": 1 }))
        .send()
        .await
        .unwrap();
    let once: serde_json::Value = r.json().await.unwrap();
    assert_eq!(once["maxUses"], 1);
    let once_code = once["code"].as_str().unwrap().to_string();

    // Timed, already past its moment: a second in the past is refused outright.
    let r = c
        .post(format!("{base}/api/servers/{server_id}/invites"))
        .bearer_auth(&token_a)
        .json(&serde_json::json!({ "expiresInSeconds": 1 }))
        .send()
        .await
        .unwrap();
    let timed: serde_json::Value = r.json().await.unwrap();
    assert!(timed["expiresAt"].is_string());
    let timed_code = timed["code"].as_str().unwrap().to_string();

    // The list is the owner's alone.
    let r = c
        .get(format!("{base}/api/servers/{server_id}/invites"))
        .bearer_auth(&token_b)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);

    let r = c
        .get(format!("{base}/api/servers/{server_id}/invites"))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap();
    let list: serde_json::Value = r.json().await.unwrap();
    assert_eq!(list.as_array().unwrap().len(), 3, "newest first, all three");

    // A preview tells the joiner where the link leads.
    let r = c
        .get(format!("{base}/api/invites/{once_code}"))
        .bearer_auth(&token_b)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let preview: serde_json::Value = r.json().await.unwrap();
    assert_eq!(preview["serverId"].as_str().unwrap(), server_id);
    assert_eq!(preview["alreadyMember"], true, "bob is already in this server");

    // Bob is in already, so use a third account to spend the one-off link.
    let r = c
        .post(format!("{base}/api/auth/register"))
        .json(&serde_json::json!({ "username": "carol", "email": "carol@x.com", "password": "pw123" }))
        .send()
        .await
        .unwrap();
    let token_c = r.json::<serde_json::Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();

    let r = c
        .post(format!("{base}/api/servers/join"))
        .bearer_auth(&token_c)
        .json(&serde_json::json!({ "inviteCode": once_code }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "the one-off link works once");

    // And is spent: a fourth account gets 410 rather than joining.
    let r = c
        .post(format!("{base}/api/auth/register"))
        .json(&serde_json::json!({ "username": "dave", "email": "dave@x.com", "password": "pw123" }))
        .send()
        .await
        .unwrap();
    let token_d = r.json::<serde_json::Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();
    let r = c
        .post(format!("{base}/api/servers/join"))
        .bearer_auth(&token_d)
        .json(&serde_json::json!({ "inviteCode": once_code }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 410, "used up");

    // The timed one has a second to live; wait it out and it is gone too.
    tokio::time::sleep(std::time::Duration::from_millis(1300)).await;
    let r = c
        .post(format!("{base}/api/servers/join"))
        .bearer_auth(&token_d)
        .json(&serde_json::json!({ "inviteCode": timed_code }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 410, "expired");

    // Revoking: the owner switches the permanent link off, and it stops working.
    let permanent_id = permanent["id"].as_str().unwrap();
    let permanent_code = permanent["code"].as_str().unwrap().to_string();
    let r = c
        .delete(format!("{base}/api/servers/invites/{permanent_id}"))
        .bearer_auth(&token_b)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403, "not bob's to revoke");

    let r = c
        .delete(format!("{base}/api/servers/invites/{permanent_id}"))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    let r = c
        .post(format!("{base}/api/servers/join"))
        .bearer_auth(&token_d)
        .json(&serde_json::json!({ "inviteCode": permanent_code }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 410, "revoked");

    // The list now marks it inactive rather than dropping it.
    let r = c
        .get(format!("{base}/api/servers/{server_id}/invites"))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap();
    let list: serde_json::Value = r.json().await.unwrap();
    let revoked = list
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"].as_str() == Some(permanent_id))
        .unwrap();
    assert_eq!(revoked["active"], false);
    assert!(revoked["revokedAt"].is_string());

    // The one-off shows its use.
    let spent = list
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["code"].as_str() == Some(once_code.as_str()))
        .unwrap();
    assert_eq!(spent["uses"], 1);
    assert_eq!(spent["active"], false);

    // --- leaving and deleting a server ---
    // The owner cannot leave; that is what deleting is for.
    let r = c
        .delete(format!("{base}/api/servers/{server_id}/leave"))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);

    // A member can, and then no longer sees the server.
    let r = c
        .delete(format!("{base}/api/servers/{server_id}/leave"))
        .bearer_auth(&token_b)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    let r = c
        .get(format!("{base}/api/servers"))
        .bearer_auth(&token_b)
        .send()
        .await
        .unwrap();
    let bobs: serde_json::Value = r.json().await.unwrap();
    assert!(
        !bobs.as_array().unwrap().iter().any(|s| s["id"].as_str() == Some(server_id.as_str())),
        "left servers must be gone from the list"
    );

    // Leaving twice is refused rather than silently succeeding.
    let r = c
        .delete(format!("{base}/api/servers/{server_id}/leave"))
        .bearer_auth(&token_b)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);

    // Deleting belongs to the owner alone.
    let r = c
        .delete(format!("{base}/api/servers/{server_id}"))
        .bearer_auth(&token_c)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);

    let r = c
        .delete(format!("{base}/api/servers/{server_id}"))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    // Gone for its owner, and its channels with it.
    let r = c
        .get(format!("{base}/api/servers"))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap();
    let mine: serde_json::Value = r.json().await.unwrap();
    assert!(
        !mine.as_array().unwrap().iter().any(|s| s["id"].as_str() == Some(server_id.as_str())),
        "the deleted server must be gone"
    );
    let r = c
        .get(format!("{base}/api/servers/channels/{channel_id}/messages"))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404, "its channels went with it");

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

/// Arbitrary files may be attached now, so what /uploads hands back matters: media
/// stays inline for the chat to render, everything else is a download.
#[tokio::test]
async fn uploads_are_served_as_downloads_except_media() {
    let base = spawn_server().await;
    let c = client();

    let r = c
        .post(format!("{base}/api/auth/register"))
        .json(&serde_json::json!({ "username": "carol", "email": "c@x.com", "password": "pw123" }))
        .send()
        .await
        .unwrap();
    let token = r.json::<serde_json::Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();

    let put = |name: &'static str, bytes: &'static [u8]| {
        let c = c.clone();
        let base = base.clone();
        let token = token.clone();
        async move {
            let data = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes);
            let r = c
                .post(format!("{base}/api/upload"))
                .bearer_auth(&token)
                .json(&serde_json::json!({ "filename": name, "fileData": data }))
                .send()
                .await
                .unwrap();
            assert_eq!(r.status(), 200);
            r.json::<serde_json::Value>().await.unwrap()["url"]
                .as_str()
                .unwrap()
                .to_string()
        }
    };

    // The upload handler builds the URL from the Host header; the port is what
    // matters, and it is already this test's server.
    let txt_url = put("notes.txt", b"plain text").await;
    let png_url = put("shot.png", &[0x89, b'P', b'N', b'G']).await;
    let svg_url = put("logo.svg", b"<svg xmlns='http://www.w3.org/2000/svg'/>").await;

    let fetch = |url: String| {
        let c = c.clone();
        let base = base.clone();
        async move {
            let path = url.split("/uploads/").nth(1).unwrap().to_string();
            let r = c
                .get(format!("{base}/uploads/{path}"))
                .send()
                .await
                .unwrap();
            assert_eq!(r.status(), 200);
            let h = r.headers();
            (
                h.get("content-disposition")
                    .map(|v| v.to_str().unwrap().to_string()),
                h.get("x-content-type-options")
                    .map(|v| v.to_str().unwrap().to_string()),
                path,
            )
        }
    };

    let (disp, nosniff, txt_path) = fetch(txt_url).await;
    // Saved under the name that was picked, without the stored timestamp.
    assert_eq!(
        disp.as_deref(),
        Some("attachment; filename*=UTF-8''notes.txt")
    );
    assert_eq!(nosniff.as_deref(), Some("nosniff"));

    let (disp, _, png_path) = fetch(png_url).await;
    assert_eq!(disp, None, "a picture must stay inline for the chat");

    let (disp, nosniff, svg_path) = fetch(svg_url).await;
    assert!(
        disp.is_some() && nosniff.is_some(),
        "svg can carry script, so it is a download despite being an image"
    );

    for p in [txt_path, png_path, svg_path] {
        let _ = std::fs::remove_file(std::path::Path::new("uploads").join(p));
    }
}
