//! Socket.io end-to-end test using a real Rust socket.io client
//! (rust_socketio) against the actual socketioxide server, over the wire.

use std::time::Duration;

use axum::Router;
use rust_socketio::asynchronous::{Client, ClientBuilder};
use rust_socketio::Payload;
use serde_json::{json, Value};
use socketioxide::SocketIo;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tower_http::services::ServeDir;

use crate::{db, realtime, routes, state::AppState};

/// Returns the base URL and the pool, so a test can look straight at a table
/// when what it needs to check has no endpoint (reaction rows, for one).
async fn spawn_full_server() -> (String, crate::db::Db) {
    let path = std::env::temp_dir().join(format!("akami_sock_{}.db", db::new_id()));
    let url = format!("sqlite:{}", path.display());
    let pool = db::connect(&url).await.unwrap();
    let pool_for_tests = pool.clone();

    let cfg = crate::config::Config {
        port: 0,
        jwt_secret: "test-secret".into(),
        dm_encryption_key: "test-dm-key".into(),
        database_url: url,
    };
    let st = AppState::new(pool, cfg);

    let (layer, io) = SocketIo::builder().with_state(st.clone()).build_layer();
    realtime::register(&io, &st);

    let app = Router::new()
        .merge(routes::api_router())
        .nest_service("/uploads", ServeDir::new("uploads"))
        .layer(layer)
        .with_state(st);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), pool_for_tests)
}

async fn register(c: &reqwest::Client, base: &str, name: &str) -> (String, String) {
    let r: Value = c
        .post(format!("{base}/api/auth/register"))
        .json(&json!({ "username": name, "email": format!("{name}@x.com"), "password": "pw" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    (
        r["token"].as_str().unwrap().to_string(),
        r["user"]["id"].as_str().unwrap().to_string(),
    )
}

/// Connects a socket.io client that forwards every named event we care about
/// into an mpsc channel as `(event_name, first_payload_value)`.
async fn connect_client(base: &str, token: &str) -> (Client, UnboundedReceiver<(String, Value)>) {
    let (tx, rx) = unbounded_channel::<(String, Value)>();

    let events = [
        "message_received",
        "message_updated",
        "message_deleted",
        "reaction_updated",
        "pin_updated",
        "dm_received",
        "dm_updated",
        "dm_deleted",
        "status_changed",
        "voice_state_sync",
        "user_profile_updated",
        "user_joined_voice",
        "user_left_voice",
        "dm_call_incoming",
        "dm_call_accepted",
        "dm_call_declined",
        "dm_call_hungup",
    ];

    let mut builder = ClientBuilder::new(base.to_string())
        .namespace("/")
        .auth(json!({ "token": token }));

    for ev in events {
        let tx: UnboundedSender<(String, Value)> = tx.clone();
        let ev_name = ev.to_string();
        builder = builder.on(ev, move |payload: Payload, _c: Client| {
            let tx = tx.clone();
            let ev_name = ev_name.clone();
            Box::pin(async move {
                if let Payload::Text(vals) = payload {
                    let first = vals.into_iter().next().unwrap_or(Value::Null);
                    let _ = tx.send((ev_name, first));
                }
            })
        });
    }

    let client = builder.connect().await.expect("socket connect failed");
    (client, rx)
}

/// Waits up to 5s for a specific event, ignoring others.
async fn wait_for(rx: &mut UnboundedReceiver<(String, Value)>, event: &str) -> Value {
    let deadline = Duration::from_secs(5);
    loop {
        let (name, val) = tokio::time::timeout(deadline, rx.recv())
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for `{event}`"))
            .expect("channel closed");
        if name == event {
            return val;
        }
    }
}

#[tokio::test]
async fn socket_messaging_and_dm() {
    let (base, pool) = spawn_full_server().await;
    let http = reqwest::Client::new();

    let (token_a, _id_a) = register(&http, &base, "alice").await;
    let (token_b, id_b) = register(&http, &base, "bob").await;

    // Alice makes a server+channel; Bob joins via invite so both are members.
    let server: Value = http
        .post(format!("{base}/api/servers"))
        .bearer_auth(&token_a)
        .json(&json!({ "name": "S" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let channel_id = server["channels"][0]["id"].as_str().unwrap().to_string();
    let invite = server["inviteCode"].as_str().unwrap().to_string();

    http.post(format!("{base}/api/servers/join"))
        .bearer_auth(&token_b)
        .json(&json!({ "inviteCode": invite }))
        .send()
        .await
        .unwrap();

    // Connect both sockets.
    let (sock_a, mut rx_a) = connect_client(&base, &token_a).await;
    let (sock_b, mut rx_b) = connect_client(&base, &token_b).await;

    // Readiness barrier: the server emits `voice_state_sync` to each socket on
    // connect, so receiving it proves the namespace is fully connected before
    // we start emitting (otherwise early emits can race the CONNECT handshake).
    wait_for(&mut rx_a, "voice_state_sync").await;
    wait_for(&mut rx_b, "voice_state_sync").await;

    // Both join the channel room.
    sock_a
        .emit("join_channel", Payload::Text(vec![json!(channel_id)]))
        .await
        .unwrap();
    sock_b
        .emit("join_channel", Payload::Text(vec![json!(channel_id)]))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    // --- channel message ---
    sock_a
        .emit(
            "send_message",
            Payload::Text(vec![json!({ "channelId": channel_id, "content": "привет мир" })]),
        )
        .await
        .unwrap();

    let msg_a = wait_for(&mut rx_a, "message_received").await;
    let msg_b = wait_for(&mut rx_b, "message_received").await;
    assert_eq!(msg_a["content"], "привет мир");
    assert_eq!(msg_b["content"], "привет мир");
    assert_eq!(msg_b["user"]["username"], "alice");
    assert_eq!(msg_b["channelId"], channel_id);
    assert!(msg_b["components"].is_null());
    // Date must be Prisma-style `...Z`.
    let created = msg_b["createdAt"].as_str().unwrap();
    assert!(created.ends_with('Z') && created.len() == 24, "bad date: {created}");

    // --- edit message with buttons ---
    let msg_id = msg_a["id"].as_str().unwrap().to_string();
    sock_a
        .emit(
            "edit_message",
            Payload::Text(vec![json!({
                "messageId": msg_id,
                "content": "исправлено",
                "components": [{ "type": "button", "customId": "ok", "label": "OK" }]
            })]),
        )
        .await
        .unwrap();
    let upd = wait_for(&mut rx_b, "message_updated").await;
    assert_eq!(upd["content"], "исправлено");
    assert_eq!(upd["components"][0]["customId"], "ok");

    // --- direct message (encrypted at rest, decrypted in payload) ---
    sock_a
        .emit(
            "send_dm",
            Payload::Text(vec![json!({ "receiverId": id_b, "content": "секретное сообщение" })]),
        )
        .await
        .unwrap();

    let dm_a = wait_for(&mut rx_a, "dm_received").await;
    let dm_b = wait_for(&mut rx_b, "dm_received").await;
    assert_eq!(dm_a["content"], "секретное сообщение");
    assert_eq!(dm_b["content"], "секретное сообщение");
    assert_eq!(dm_b["sender"]["username"], "alice");
    assert_eq!(dm_b["receiver"]["username"], "bob");

    // Confirm it is actually stored encrypted (REST history decrypts it back).
    let history: Value = http
        .get(format!("{base}/api/users/dms/{id_b}"))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(history[0]["content"], "секретное сообщение");

    // --- edit that direct message ---
    let dm_id = dm_a["id"].as_str().unwrap().to_string();
    // A fresh message carries no updatedAt at all; that absence is what the client
    // reads as "never edited".
    assert!(dm_a.get("updatedAt").is_none());

    sock_a
        .emit(
            "edit_dm",
            Payload::Text(vec![json!({ "messageId": dm_id, "content": "исправленный секрет" })]),
        )
        .await
        .unwrap();
    // Both sides hear about it, not just the author.
    let edited_b = wait_for(&mut rx_b, "dm_updated").await;
    let edited_a = wait_for(&mut rx_a, "dm_updated").await;
    assert_eq!(edited_b["content"], "исправленный секрет");
    assert_eq!(edited_a["id"].as_str().unwrap(), dm_id);
    assert!(edited_b["updatedAt"].is_string());

    // Still encrypted at rest, and the history now reports the edit.
    let history: Value = http
        .get(format!("{base}/api/users/dms/{id_b}"))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(history[0]["content"], "исправленный секрет");
    assert!(history[0]["updatedAt"].is_string());

    // Bob may not edit Alice's message: nothing changes.
    sock_b
        .emit(
            "edit_dm",
            Payload::Text(vec![json!({ "messageId": dm_id, "content": "подделка" })]),
        )
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    let history: Value = http
        .get(format!("{base}/api/users/dms/{id_b}"))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(history[0]["content"], "исправленный секрет");

    // --- replying ---
    sock_b
        .emit(
            "send_message",
            Payload::Text(vec![json!({
                "channelId": channel_id,
                "content": "отвечаю",
                "replyToId": msg_id,
            })]),
        )
        .await
        .unwrap();
    let reply = wait_for(&mut rx_a, "message_received").await;
    assert_eq!(reply["replyToId"].as_str().unwrap(), msg_id);
    assert_eq!(reply["replyTo"]["username"], "alice");
    assert_eq!(reply["replyTo"]["content"], "исправлено");
    let reply_id = reply["id"].as_str().unwrap().to_string();

    // A message from another channel cannot be quoted: the reference is dropped
    // rather than pointing at something the readers cannot see.
    sock_b
        .emit(
            "send_message",
            Payload::Text(vec![json!({
                "channelId": channel_id,
                "content": "чужая цитата",
                "replyToId": dm_id,
            })]),
        )
        .await
        .unwrap();
    let stray = wait_for(&mut rx_a, "message_received").await;
    assert!(stray["replyToId"].is_null());
    assert!(stray.get("replyTo").is_none() || stray["replyTo"].is_null());

    // --- reactions ---
    sock_b
        .emit(
            "toggle_reaction",
            Payload::Text(vec![json!({ "messageId": msg_id, "emoji": "👍" })]),
        )
        .await
        .unwrap();
    // Everyone in the channel hears it, the author included. Both queues are read
    // every time, so a leftover event cannot be mistaken for the next one.
    let react = wait_for(&mut rx_a, "reaction_updated").await;
    let mirrored = wait_for(&mut rx_b, "reaction_updated").await;
    assert_eq!(react["messageId"].as_str().unwrap(), msg_id);
    assert_eq!(react["reactions"][0]["emoji"], "👍");
    assert_eq!(react["reactions"][0]["users"][0].as_str().unwrap(), id_b);
    assert_eq!(mirrored["reactions"][0]["emoji"], "👍");

    // Alice adds a different one: two groups, in the order they arrived.
    sock_a
        .emit(
            "toggle_reaction",
            Payload::Text(vec![json!({ "messageId": msg_id, "emoji": "🎉" })]),
        )
        .await
        .unwrap();
    let react = wait_for(&mut rx_b, "reaction_updated").await;
    let _ = wait_for(&mut rx_a, "reaction_updated").await;
    assert_eq!(react["reactions"].as_array().unwrap().len(), 2);
    assert_eq!(react["reactions"][1]["emoji"], "🎉");

    // One reaction each: bob picking a different emoji replaces his own rather than
    // adding a second, and does not touch alice's.
    sock_b
        .emit(
            "toggle_reaction",
            Payload::Text(vec![json!({ "messageId": msg_id, "emoji": "😮" })]),
        )
        .await
        .unwrap();
    let react = wait_for(&mut rx_a, "reaction_updated").await;
    let _ = wait_for(&mut rx_b, "reaction_updated").await;
    let groups = react["reactions"].as_array().unwrap();
    assert_eq!(groups.len(), 2, "alice's 🎉 plus bob's new one, not three");
    let bobs: Vec<&str> = groups
        .iter()
        .filter(|g| {
            g["users"]
                .as_array()
                .unwrap()
                .iter()
                .any(|u| u.as_str() == Some(id_b.as_str()))
        })
        .map(|g| g["emoji"].as_str().unwrap())
        .collect();
    assert_eq!(bobs, vec!["😮"], "the 👍 must be gone");

    // Back to 👍 for the rest of the test.
    sock_b
        .emit(
            "toggle_reaction",
            Payload::Text(vec![json!({ "messageId": msg_id, "emoji": "👍" })]),
        )
        .await
        .unwrap();
    let _ = wait_for(&mut rx_a, "reaction_updated").await;
    let _ = wait_for(&mut rx_b, "reaction_updated").await;

    // The same emoji again from the same person takes it back.
    sock_b
        .emit(
            "toggle_reaction",
            Payload::Text(vec![json!({ "messageId": msg_id, "emoji": "👍" })]),
        )
        .await
        .unwrap();
    let react = wait_for(&mut rx_a, "reaction_updated").await;
    let _ = wait_for(&mut rx_b, "reaction_updated").await;
    assert_eq!(react["reactions"].as_array().unwrap().len(), 1);
    assert_eq!(react["reactions"][0]["emoji"], "🎉");

    // History carries them, so a reload does not lose them.
    let msgs: Value = http
        .get(format!("{base}/api/servers/channels/{channel_id}/messages"))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let stored = msgs
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"].as_str() == Some(&msg_id))
        .unwrap();
    assert_eq!(stored["reactions"][0]["emoji"], "🎉");
    let stored_reply = msgs
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"].as_str() == Some(&reply_id))
        .unwrap();
    assert_eq!(stored_reply["replyTo"]["content"], "исправлено");

    // A reaction on a direct message reaches both sides.
    sock_b
        .emit(
            "toggle_reaction",
            Payload::Text(vec![json!({ "messageId": dm_id, "emoji": "❤️" })]),
        )
        .await
        .unwrap();
    let dm_react_a = wait_for(&mut rx_a, "reaction_updated").await;
    let dm_react_b = wait_for(&mut rx_b, "reaction_updated").await;
    assert_eq!(dm_react_a["messageId"].as_str().unwrap(), dm_id);
    assert_eq!(dm_react_b["reactions"][0]["emoji"], "❤️");
    let history: Value = http
        .get(format!("{base}/api/users/dms/{id_b}"))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(history[0]["reactions"][0]["emoji"], "❤️");

    // A reply in a conversation, live and in the history — the quote is decrypted
    // there just like the message itself is.
    sock_a
        .emit(
            "send_dm",
            Payload::Text(vec![json!({
                "receiverId": id_b,
                "content": "отвечаю в лс",
                "replyToId": dm_id,
            })]),
        )
        .await
        .unwrap();
    let dm_reply_b = wait_for(&mut rx_b, "dm_received").await;
    let _ = wait_for(&mut rx_a, "dm_received").await;
    assert_eq!(dm_reply_b["replyTo"]["username"], "alice");
    assert_eq!(dm_reply_b["replyTo"]["content"], "исправленный секрет");
    let dm_reply_id = dm_reply_b["id"].as_str().unwrap().to_string();

    let history: Value = http
        .get(format!("{base}/api/users/dms/{id_b}"))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let stored_dm_reply = history
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"].as_str() == Some(&dm_reply_id))
        .unwrap();
    assert_eq!(
        stored_dm_reply["replyTo"]["content"], "исправленный секрет",
        "the history must carry the quote, under a camelCase key"
    );

    // --- forwarding ---
    // The client names the source message, not the author, so the label cannot be
    // faked: the server looks up who wrote it.
    sock_b
        .emit(
            "send_message",
            Payload::Text(vec![json!({
                "channelId": channel_id,
                "content": "исправлено",
                "forwardedFromId": msg_id,
            })]),
        )
        .await
        .unwrap();
    let fwd = wait_for(&mut rx_a, "message_received").await;
    assert_eq!(fwd["forwardedFromUser"]["username"], "alice");
    let fwd_id = fwd["id"].as_str().unwrap().to_string();

    // An id that matches nothing leaves no mark at all.
    sock_b
        .emit(
            "send_message",
            Payload::Text(vec![json!({
                "channelId": channel_id,
                "content": "не переслано",
                "forwardedFromId": "no-such-message",
            })]),
        )
        .await
        .unwrap();
    let plain = wait_for(&mut rx_a, "message_received").await;
    assert!(plain["forwardedFrom"].is_null());
    assert!(plain.get("forwardedFromUser").is_none() || plain["forwardedFromUser"].is_null());

    let msgs: Value = http
        .get(format!("{base}/api/servers/channels/{channel_id}/messages"))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let stored_fwd = msgs
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"].as_str() == Some(&fwd_id))
        .unwrap();
    assert_eq!(stored_fwd["forwardedFromUser"]["username"], "alice");

    // The same in a conversation.
    sock_a
        .emit(
            "send_dm",
            Payload::Text(vec![json!({
                "receiverId": id_b,
                "content": "пересылаю в лс",
                "forwardedFromId": dm_id,
            })]),
        )
        .await
        .unwrap();
    let dm_fwd = wait_for(&mut rx_b, "dm_received").await;
    let _ = wait_for(&mut rx_a, "dm_received").await;
    assert_eq!(dm_fwd["forwardedFromUser"]["username"], "alice");

    // --- pinning ---
    // Not the author this time: bob pins alice's message, and both sides hear it.
    sock_b
        .emit(
            "toggle_pin",
            Payload::Text(vec![json!({ "messageId": msg_id })]),
        )
        .await
        .unwrap();
    let pinned = wait_for(&mut rx_a, "pin_updated").await;
    let _ = wait_for(&mut rx_b, "pin_updated").await;
    assert_eq!(pinned["messageId"].as_str().unwrap(), msg_id);
    assert!(pinned["pinnedAt"].is_string());

    let msgs: Value = http
        .get(format!("{base}/api/servers/channels/{channel_id}/messages"))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let stored = msgs
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"].as_str() == Some(&msg_id))
        .unwrap();
    assert!(stored["pinnedAt"].is_string(), "the history must carry the pin");

    // Pinning again unpins, and the field goes away rather than turning null.
    sock_b
        .emit(
            "toggle_pin",
            Payload::Text(vec![json!({ "messageId": msg_id })]),
        )
        .await
        .unwrap();
    let unpinned = wait_for(&mut rx_a, "pin_updated").await;
    let _ = wait_for(&mut rx_b, "pin_updated").await;
    assert!(unpinned["pinnedAt"].is_null());
    let msgs: Value = http
        .get(format!("{base}/api/servers/channels/{channel_id}/messages"))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let stored = msgs
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"].as_str() == Some(&msg_id))
        .unwrap();
    assert!(stored.get("pinnedAt").is_none());

    // A direct message pins for both of its two people.
    sock_b
        .emit(
            "toggle_pin",
            Payload::Text(vec![json!({ "messageId": dm_id })]),
        )
        .await
        .unwrap();
    let dm_pin = wait_for(&mut rx_a, "pin_updated").await;
    let _ = wait_for(&mut rx_b, "pin_updated").await;
    assert_eq!(dm_pin["messageId"].as_str().unwrap(), dm_id);
    let history: Value = http
        .get(format!("{base}/api/users/dms/{id_b}"))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(history[0]["pinnedAt"].is_string());

    // --- deleting: only the author, and both sides hear about it ---
    // Bob cannot delete Alice's direct message.
    sock_b
        .emit(
            "delete_dm",
            Payload::Text(vec![json!({ "messageId": dm_id })]),
        )
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    let history: Value = http
        .get(format!("{base}/api/users/dms/{id_b}"))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(history.as_array().unwrap().len(), 3, "not the author's to delete");

    sock_a
        .emit(
            "delete_dm",
            Payload::Text(vec![json!({ "messageId": dm_id })]),
        )
        .await
        .unwrap();
    let gone_b = wait_for(&mut rx_b, "dm_deleted").await;
    let gone_a = wait_for(&mut rx_a, "dm_deleted").await;
    assert_eq!(gone_b["id"].as_str().unwrap(), dm_id);
    assert_eq!(gone_a["id"].as_str().unwrap(), dm_id);
    let history: Value = http
        .get(format!("{base}/api/users/dms/{id_b}"))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ids: Vec<&str> = history
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap())
        .collect();
    assert!(!ids.contains(&dm_id.as_str()), "row must be gone");

    // The reactions went with it, rather than lingering for a future id to inherit.
    let left: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM Reaction WHERE messageId = ?")
        .bind(&dm_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(left.0, 0);

    // Same for the channel message.
    sock_b
        .emit(
            "delete_message",
            Payload::Text(vec![json!({ "messageId": msg_id })]),
        )
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    let msgs: Value = http
        .get(format!("{base}/api/servers/channels/{channel_id}/messages"))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    // Alice's original plus what bob sent while testing replies and forwards.
    assert_eq!(msgs.as_array().unwrap().len(), 5, "not the author's to delete");

    sock_a
        .emit(
            "delete_message",
            Payload::Text(vec![json!({ "messageId": msg_id })]),
        )
        .await
        .unwrap();
    let gone = wait_for(&mut rx_b, "message_deleted").await;
    assert_eq!(gone["id"].as_str().unwrap(), msg_id);
    let msgs: Value = http
        .get(format!("{base}/api/servers/channels/{channel_id}/messages"))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let left: Vec<&str> = msgs
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap())
        .collect();
    assert!(!left.contains(&msg_id.as_str()), "row must be gone");

    // The reply outlives what it answered; its quote simply stops resolving.
    let orphan = msgs
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"].as_str() == Some(&reply_id))
        .unwrap();
    assert_eq!(orphan["replyToId"].as_str().unwrap(), msg_id);
    assert!(orphan.get("replyTo").is_none());

    // Its reactions went too.
    let stale: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM Reaction WHERE messageId = ?")
        .bind(&msg_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(stale.0, 0);

    // --- voice: alice joins, both get a voice_state_sync with her entry ---
    sock_a
        .emit(
            "join_voice",
            Payload::Text(vec![json!({ "channelId": channel_id })]),
        )
        .await
        .unwrap();
    let sync = wait_for(&mut rx_b, "voice_state_sync").await;
    let entry = &sync[&channel_id][0];
    assert_eq!(entry["username"], "alice");
    assert_eq!(entry["mute"], false);

    sock_a.disconnect().await.ok();
    sock_b.disconnect().await.ok();
}

/// Room-scoped emits are futures: dropping one instead of awaiting it sends
/// nothing, and the channel keeps working through `voice_state_sync` — which is
/// exactly what made the silence hard to notice. These are the events that only
/// ever travel that way.
#[tokio::test]
async fn socket_room_broadcasts_reach_peers() {
    let (base, _pool) = spawn_full_server().await;
    let http = reqwest::Client::new();

    let (token_a, id_a) = register(&http, &base, "dialer").await;
    let (token_b, id_b) = register(&http, &base, "callee").await;

    let server: Value = http
        .post(format!("{base}/api/servers"))
        .bearer_auth(&token_a)
        .json(&json!({ "name": "S" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let channel_id = server["channels"][0]["id"].as_str().unwrap().to_string();
    let invite = server["inviteCode"].as_str().unwrap().to_string();
    http.post(format!("{base}/api/servers/join"))
        .bearer_auth(&token_b)
        .json(&json!({ "inviteCode": invite }))
        .send()
        .await
        .unwrap();

    let (sock_a, mut rx_a) = connect_client(&base, &token_a).await;
    let (sock_b, mut rx_b) = connect_client(&base, &token_b).await;
    wait_for(&mut rx_a, "voice_state_sync").await;
    wait_for(&mut rx_b, "voice_state_sync").await;

    // --- joining and leaving a voice channel, as the peer already inside sees it ---
    sock_a
        .emit(
            "join_voice",
            Payload::Text(vec![json!({ "channelId": channel_id })]),
        )
        .await
        .unwrap();
    wait_for(&mut rx_b, "voice_state_sync").await;

    sock_b
        .emit(
            "join_voice",
            Payload::Text(vec![json!({ "channelId": channel_id })]),
        )
        .await
        .unwrap();
    let joined = wait_for(&mut rx_a, "user_joined_voice").await;
    assert_eq!(joined["userId"].as_str().unwrap(), id_b);

    sock_b
        .emit(
            "leave_voice",
            Payload::Text(vec![json!({ "channelId": channel_id })]),
        )
        .await
        .unwrap();
    let left = wait_for(&mut rx_a, "user_left_voice").await;
    assert_eq!(left["userId"].as_str().unwrap(), id_b);

    // --- a direct call, ringing to hangup ---
    sock_a
        .emit(
            "dm_call_dial",
            Payload::Text(vec![json!({
                "receiverId": id_b,
                "roomId": "room-1",
                "caller": { "id": id_a, "username": "dialer" },
            })]),
        )
        .await
        .unwrap();
    let ringing = wait_for(&mut rx_b, "dm_call_incoming").await;
    assert_eq!(ringing["roomId"], "room-1");
    assert_eq!(ringing["caller"]["username"], "dialer");

    sock_b
        .emit(
            "dm_call_accept",
            Payload::Text(vec![json!({ "callerId": id_a, "roomId": "room-1" })]),
        )
        .await
        .unwrap();
    let accepted = wait_for(&mut rx_a, "dm_call_accepted").await;
    assert_eq!(accepted["roomId"], "room-1");

    sock_a
        .emit(
            "dm_call_hangup",
            Payload::Text(vec![json!({ "receiverId": id_b })]),
        )
        .await
        .unwrap();
    wait_for(&mut rx_b, "dm_call_hungup").await;

    // --- and the same call declined instead ---
    sock_a
        .emit(
            "dm_call_dial",
            Payload::Text(vec![json!({
                "receiverId": id_b,
                "roomId": "room-2",
                "caller": { "id": id_a, "username": "dialer" },
            })]),
        )
        .await
        .unwrap();
    wait_for(&mut rx_b, "dm_call_incoming").await;
    sock_b
        .emit(
            "dm_call_decline",
            Payload::Text(vec![json!({ "callerId": id_a })]),
        )
        .await
        .unwrap();
    wait_for(&mut rx_a, "dm_call_declined").await;

    sock_a.disconnect().await.ok();
    sock_b.disconnect().await.ok();
}
