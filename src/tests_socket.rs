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

async fn spawn_full_server() -> String {
    let path = std::env::temp_dir().join(format!("akami_sock_{}.db", db::new_id()));
    let url = format!("sqlite:{}", path.display());
    let pool = db::connect(&url).await.unwrap();

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
    format!("http://{addr}")
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
        "dm_received",
        "status_changed",
        "voice_state_sync",
        "user_profile_updated",
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
    let base = spawn_full_server().await;
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
