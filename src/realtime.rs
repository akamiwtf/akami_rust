//! Socket.io layer (socketioxide), a faithful port of the `io.on('connection')`
//! handlers in server/src/index.js. Event names, payloads and room semantics
//! match the Node server so the existing socket.io-client keeps working.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::{json, Value};
use socketioxide::extract::{Data, SocketRef, State};
use socketioxide::SocketIo;
use sqlx::FromRow;

use crate::auth;
use crate::db::{new_id, now_db, sql};
use crate::models::{DirectMessage, Message, PublicUser, PUBLIC_USER_COLS};
use crate::state::{AppState, Broadcaster, EmitTarget};
use crate::voice::now_ms;

/// Per-socket authenticated user id, kept in socket extensions.
#[derive(Clone)]
struct SocketUser(String);

#[derive(Deserialize)]
struct AuthData {
    #[serde(default)]
    token: Option<String>,
}

/// messageUserSelect from index.js (has displayName+banner, no pronouns).
const MSG_USER_COLS: &str = "id, username, displayName, avatar, banner, status, bio, badges, \
    customStatus, socials, createdAt";

#[derive(FromRow, serde::Serialize)]
#[serde(rename_all = "camelCase")]
#[sqlx(rename_all = "camelCase")]
struct MsgUser {
    id: String,
    username: String,
    display_name: Option<String>,
    avatar: Option<String>,
    banner: Option<String>,
    status: String,
    bio: String,
    badges: String,
    custom_status: String,
    socials: String,
    #[serde(with = "crate::models::prisma_date")]
    created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(FromRow)]
#[sqlx(rename_all = "camelCase")]
struct VoiceProfile {
    username: String,
    avatar: Option<String>,
    status: String,
    bio: String,
    pronouns: String,
    is_bot: bool,
}

/// Installs the io-backed broadcaster and registers the `/` namespace.
pub fn register(io: &SocketIo, st: &AppState) {
    let io_for_bc = io.clone();
    let broadcaster: Broadcaster = Arc::new(move |target, event: &str, payload: Value| {
        let op = match target {
            EmitTarget::All => io_for_bc.broadcast(),
            EmitTarget::Room(room) => io_for_bc.to(room.to_string()),
        };
        let event = event.to_string();
        // `payload` must outlive the emit future, so own it inside the task.
        tokio::spawn(async move {
            let _ = op.emit(event, &payload).await;
        });
    });
    st.set_broadcaster(broadcaster);

    io.ns("/", on_connect);
}

macro_rules! on_event {
    ($socket:expr, $st:expr, $event:literal, $handler:path) => {{
        let st = $st.clone();
        $socket.on(
            $event,
            move |s: SocketRef, Data(v): Data<Value>| {
                let st = st.clone();
                async move {
                    $handler(st, s, v).await;
                }
            },
        );
    }};
}

async fn on_connect(socket: SocketRef, Data(auth_data): Data<AuthData>, State(st): State<AppState>) {
    let Some(user_id) = auth_data
        .token
        .as_deref()
        .and_then(|t| auth::verify(t, &st.config.jwt_secret))
    else {
        let _ = socket.disconnect();
        return;
    };

    // Ensure the user still exists and flip them online.
    let updated = sqlx::query("UPDATE User SET status = 'online' WHERE id = ?")
        .bind(&user_id)
        .execute(&st.db)
        .await;
    match updated {
        Ok(res) if res.rows_affected() > 0 => {}
        _ => {
            let _ = socket.disconnect();
            return;
        }
    }

    socket.extensions.insert(SocketUser(user_id.clone()));
    // Personal room (userId) + a room named after our own sid, so
    // `io.to(socketId)` reaches this exact socket (socket.io compatibility).
    socket.join(user_id.clone());
    socket.join(socket.id.to_string());

    // Current voice snapshot to just this socket, then announce we're online.
    let _ = socket.emit("voice_state_sync", &st.voice.snapshot());
    // Sync every active rich-presence to this socket so late joiners see games.
    let _ = socket.emit("presence_sync", &st.presence_snapshot());
    st.emit_all("status_changed", json!({ "userId": user_id, "status": "online" }));

    // --- channel rooms ---
    on_event!(socket, st, "join_channel", ev_join_channel);
    on_event!(socket, st, "leave_channel", ev_leave_channel);
    // --- voice ---
    on_event!(socket, st, "join_voice", ev_join_voice);
    on_event!(socket, st, "leave_voice", ev_leave_voice);
    on_event!(socket, st, "voice_signal", ev_voice_signal);
    on_event!(socket, st, "voice_state_update", ev_voice_state_update);
    on_event!(socket, st, "speaking_state_update", ev_speaking_state_update);
    on_event!(socket, st, "bot_play_audio", ev_bot_play_audio);
    on_event!(socket, st, "bot_now_playing", ev_bot_now_playing);
    on_event!(socket, st, "bot_stop_audio", ev_bot_stop_audio);
    on_event!(socket, st, "bot_volume_update", ev_bot_volume_update);
    // --- profile / messages ---
    on_event!(socket, st, "profile_updated", ev_profile_updated);
    on_event!(socket, st, "send_message", ev_send_message);
    on_event!(socket, st, "edit_message", ev_edit_message);
    on_event!(socket, st, "button_click", ev_button_click);
    // --- DMs & calls ---
    on_event!(socket, st, "send_dm", ev_send_dm);
    on_event!(socket, st, "dm_call_dial", ev_dm_call_dial);
    on_event!(socket, st, "dm_call_accept", ev_dm_call_accept);
    on_event!(socket, st, "dm_call_decline", ev_dm_call_decline);
    on_event!(socket, st, "dm_call_hangup", ev_dm_call_hangup);
    on_event!(socket, st, "update_status", ev_update_status);
    on_event!(socket, st, "update_presence", ev_update_presence);

    let st_dc = st.clone();
    socket.on_disconnect(move |s: SocketRef| {
        let st = st_dc.clone();
        async move {
            ev_disconnect(st, s).await;
        }
    });
}

fn uid(socket: &SocketRef) -> Option<String> {
    socket.extensions.get::<SocketUser>().map(|u| u.0)
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(String::from)
}

fn voice_room(channel_id: &str) -> String {
    format!("voice:{channel_id}")
}

// --- channel rooms ---

async fn ev_join_channel(_st: AppState, socket: SocketRef, v: Value) {
    if let Some(channel) = v.as_str() {
        socket.join(channel.to_string());
    }
}

async fn ev_leave_channel(_st: AppState, socket: SocketRef, v: Value) {
    if let Some(channel) = v.as_str() {
        socket.leave(channel.to_string());
    }
}

// --- voice ---

async fn ev_join_voice(st: AppState, socket: SocketRef, v: Value) {
    let Some(user_id) = uid(&socket) else { return };
    let Some(channel_id) = str_field(&v, "channelId") else { return };
    let sid = socket.id.to_string();

    let profile: Option<VoiceProfile> = sqlx::query_as(
        "SELECT username, avatar, status, bio, pronouns, isBot FROM User WHERE id = ?",
    )
    .bind(&user_id)
    .fetch_optional(&st.db)
    .await
    .ok()
    .flatten();

    let entry = match profile {
        Some(p) => json!({
            "socketId": sid, "userId": user_id, "username": p.username, "avatar": p.avatar,
            "status": p.status, "bio": p.bio, "pronouns": p.pronouns,
            "mute": false, "deaf": false, "streaming": false, "isBot": p.is_bot
        }),
        None => json!({
            "socketId": sid, "userId": user_id, "username": "Пользователь", "avatar": "",
            "status": "online", "bio": "", "pronouns": "",
            "mute": false, "deaf": false, "streaming": false, "isBot": false
        }),
    };

    // Clear any stale duplicate sessions for this socket, then add.
    st.voice.remove_socket(&sid);
    st.voice.add_user(&channel_id, entry);

    let room = voice_room(&channel_id);
    socket.join(room.clone());

    let others = st.voice.other_socket_ids(&channel_id, &sid);
    let _ = socket.emit("voice_users_list", &others);

    let _ = socket
        .to(room.clone())
        .emit("user_joined_voice", &json!({ "socketId": sid, "userId": user_id }));

    st.emit_all("voice_state_sync", st.voice.snapshot());

    if let Some(media) = st.voice.media_get(&channel_id) {
        let _ = socket.emit("bot_play_audio", &media);
    }
}

async fn ev_leave_voice(st: AppState, socket: SocketRef, v: Value) {
    let Some(user_id) = uid(&socket) else { return };
    let Some(channel_id) = str_field(&v, "channelId") else { return };
    let sid = socket.id.to_string();
    let room = voice_room(&channel_id);

    socket.leave(room.clone());
    let _ = socket
        .to(room)
        .emit("user_left_voice", &json!({ "socketId": sid, "userId": user_id }));

    if st.voice.remove_socket(&sid) {
        st.emit_all("voice_state_sync", st.voice.snapshot());
    }
}

async fn ev_voice_signal(st: AppState, socket: SocketRef, v: Value) {
    let sid = socket.id.to_string();
    let Some(target) = str_field(&v, "targetId") else { return };
    let signal = v.get("signal").cloned().unwrap_or(Value::Null);
    st.emit_to(&target, "voice_signal", json!({ "senderId": sid, "signal": signal }));
}

async fn ev_voice_state_update(st: AppState, socket: SocketRef, v: Value) {
    let Some(channel_id) = str_field(&v, "channelId") else { return };
    let sid = socket.id.to_string();
    let mute = v.get("mute").cloned().unwrap_or(Value::Bool(false));
    let deaf = v.get("deaf").cloned().unwrap_or(Value::Bool(false));
    let streaming = v.get("streaming").cloned().unwrap_or(Value::Bool(false));
    if st.voice.set_state(&channel_id, &sid, mute, deaf, streaming) {
        st.emit_all("voice_state_sync", st.voice.snapshot());
    }
}

async fn ev_speaking_state_update(st: AppState, socket: SocketRef, v: Value) {
    let sid = socket.id.to_string();
    let speaking = v.get("speaking").cloned().unwrap_or(Value::Bool(false));
    st.emit_all("user_speaking_update", json!({ "socketId": sid, "speaking": speaking }));
}

async fn ev_bot_play_audio(st: AppState, socket: SocketRef, v: Value) {
    let Some(channel_id) = str_field(&v, "channelId") else { return };
    let sid = socket.id.to_string();
    let media = json!({
        "url": v.get("url").cloned().unwrap_or(Value::Null),
        "title": v.get("title").cloned().unwrap_or(Value::Null),
        "startTime": now_ms(),
        "volume": 0.5,
        "isYouTube": v.get("isYouTube").cloned().unwrap_or(Value::Bool(false)),
        "youtubeUrl": v.get("youtubeUrl").cloned().unwrap_or(Value::Null),
        "socketId": sid,
        "label": v.get("label").cloned().unwrap_or(Value::Null),
        "icon": v.get("icon").cloned().unwrap_or(Value::Null),
    });
    st.voice.media_set(&channel_id, media.clone());
    st.emit_to(&voice_room(&channel_id), "bot_play_audio", media);
}

async fn ev_bot_now_playing(st: AppState, socket: SocketRef, v: Value) {
    let Some(channel_id) = str_field(&v, "channelId") else { return };
    let sid = socket.id.to_string();
    let media = st.voice.media_update_bar(
        &channel_id,
        &sid,
        v.get("label").cloned(),
        v.get("icon").cloned(),
    );
    st.emit_to(&voice_room(&channel_id), "bot_play_audio", media);
}

async fn ev_bot_stop_audio(st: AppState, _socket: SocketRef, v: Value) {
    let Some(channel_id) = str_field(&v, "channelId") else { return };
    st.voice.media_remove(&channel_id);
    st.emit_to(&voice_room(&channel_id), "bot_stop_audio", Value::Null);
}

async fn ev_bot_volume_update(st: AppState, socket: SocketRef, v: Value) {
    let Some(channel_id) = str_field(&v, "channelId") else { return };
    let sid = socket.id.to_string();
    let volume = v.get("volume").cloned().unwrap_or(json!(0.5));
    st.voice.media_set_volume(&channel_id, volume.clone());
    st.emit_to(
        &voice_room(&channel_id),
        "bot_volume_update",
        json!({ "socketId": sid, "volume": volume }),
    );
}

// --- profile / messages ---

async fn ev_profile_updated(st: AppState, socket: SocketRef, v: Value) {
    let sid = socket.id.to_string();
    st.emit_all("user_profile_updated", v.clone());
    if st.voice.update_profile(&sid, &v) {
        st.emit_all("voice_state_sync", st.voice.snapshot());
    }
}

async fn load_msg_user(st: &AppState, id: &str) -> Option<MsgUser> {
    sqlx::query_as(sql(format!("SELECT {MSG_USER_COLS} FROM User WHERE id = ?")))
        .bind(id)
        .fetch_optional(&st.db)
        .await
        .ok()
        .flatten()
}

async fn ev_send_message(st: AppState, socket: SocketRef, v: Value) {
    let Some(user_id) = uid(&socket) else { return };
    let Some(channel_id) = str_field(&v, "channelId") else { return };
    let content = str_field(&v, "content").unwrap_or_default();
    if content.trim().is_empty() {
        return;
    }

    // Channel must exist and the sender must be a member of its server.
    let server_id: Option<(String,)> = sqlx::query_as("SELECT serverId FROM Channel WHERE id = ?")
        .bind(&channel_id)
        .fetch_optional(&st.db)
        .await
        .ok()
        .flatten();
    let Some((server_id,)) = server_id else { return };
    let is_member: Option<(String,)> =
        sqlx::query_as("SELECT id FROM ServerMember WHERE serverId = ? AND userId = ?")
            .bind(&server_id)
            .bind(&user_id)
            .fetch_optional(&st.db)
            .await
            .ok()
            .flatten();
    if is_member.is_none() {
        return;
    }

    let components_in = v.get("components").filter(|c| c.is_array()).cloned();
    let components_str = components_in
        .as_ref()
        .map(|c| serde_json::to_string(c).unwrap());

    let id = match v.get("id").and_then(Value::as_str) {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => new_id(),
    };
    let now = now_db();

    if sqlx::query(
        "INSERT INTO Message (id, content, components, userId, channelId, createdAt, updatedAt) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&content)
    .bind(&components_str)
    .bind(&user_id)
    .bind(&channel_id)
    .bind(&now)
    .bind(&now)
    .execute(&st.db)
    .await
    .is_err()
    {
        return;
    }

    let Some(msg): Option<Message> = sqlx::query_as("SELECT * FROM Message WHERE id = ?")
        .bind(&id)
        .fetch_optional(&st.db)
        .await
        .ok()
        .flatten()
    else {
        return;
    };
    let Some(user) = load_msg_user(&st, &user_id).await else { return };

    let mut payload = serde_json::to_value(&msg).unwrap();
    payload["components"] = components_in.unwrap_or(Value::Null);
    payload["user"] = serde_json::to_value(&user).unwrap();
    st.emit_to(&channel_id, "message_received", payload);
}

async fn ev_edit_message(st: AppState, socket: SocketRef, v: Value) {
    let Some(user_id) = uid(&socket) else { return };
    let Some(message_id) = str_field(&v, "messageId") else { return };

    let Some(existing): Option<Message> = sqlx::query_as("SELECT * FROM Message WHERE id = ?")
        .bind(&message_id)
        .fetch_optional(&st.db)
        .await
        .ok()
        .flatten()
    else {
        return;
    };
    if existing.user_id != user_id {
        return;
    }

    let content_change = v.get("content").filter(|c| !c.is_null()).and_then(Value::as_str);
    // components key present at all? (null clears, array replaces, absent keeps)
    let components_present = v.get("components").is_some();
    let components_in = v.get("components").filter(|c| c.is_array()).cloned();

    let did_update = content_change.is_some() || components_present;
    if did_update {
        let mut qb: sqlx::QueryBuilder<sqlx::Sqlite> =
            sqlx::QueryBuilder::new(r#"UPDATE "Message" SET "updatedAt" = "#);
        qb.push_bind(now_db());
        if let Some(c) = content_change {
            qb.push(r#", "content" = "#).push_bind(c.to_string());
        }
        if components_present {
            let cs = components_in.as_ref().map(|c| serde_json::to_string(c).unwrap());
            qb.push(r#", "components" = "#).push_bind(cs);
        }
        qb.push(r#" WHERE id = "#).push_bind(message_id.clone());
        if qb.build().execute(&st.db).await.is_err() {
            return;
        }
    }

    let Some(updated): Option<Message> = sqlx::query_as("SELECT * FROM Message WHERE id = ?")
        .bind(&message_id)
        .fetch_optional(&st.db)
        .await
        .ok()
        .flatten()
    else {
        return;
    };
    let Some(user) = load_msg_user(&st, &updated.user_id).await else { return };

    let mut payload = serde_json::to_value(&updated).unwrap();
    payload["user"] = serde_json::to_value(&user).unwrap();
    // Only include `components` when it actually changed, so untouched buttons
    // stay as-is on clients (Node sends undefined otherwise).
    if components_present {
        payload["components"] = components_in.unwrap_or(Value::Null);
    } else if let Value::Object(map) = &mut payload {
        map.remove("components");
    }
    st.emit_to(&existing.channel_id, "message_updated", payload);
}

async fn ev_button_click(st: AppState, socket: SocketRef, v: Value) {
    let Some(user_id) = uid(&socket) else { return };
    let Some(message_id) = str_field(&v, "messageId") else { return };
    let custom_id = str_field(&v, "customId").unwrap_or_default();

    let Some(msg): Option<Message> = sqlx::query_as("SELECT * FROM Message WHERE id = ?")
        .bind(&message_id)
        .fetch_optional(&st.db)
        .await
        .ok()
        .flatten()
    else {
        return;
    };

    let clicker: Option<(String, String, Option<String>, Option<String>)> =
        sqlx::query_as("SELECT id, username, displayName, avatar FROM User WHERE id = ?")
            .bind(&user_id)
            .fetch_optional(&st.db)
            .await
            .ok()
            .flatten();
    let clicker = clicker.map(|(id, username, display_name, avatar)| {
        json!({ "id": id, "username": username, "displayName": display_name, "avatar": avatar })
    });

    st.emit_to(
        &msg.user_id,
        "button_interaction",
        json!({
            "messageId": message_id, "customId": custom_id,
            "channelId": msg.channel_id, "user": clicker
        }),
    );
}

// --- DMs & calls ---

async fn load_public_user(st: &AppState, id: &str) -> Option<PublicUser> {
    sqlx::query_as(sql(format!("SELECT {PUBLIC_USER_COLS} FROM User WHERE id = ?")))
        .bind(id)
        .fetch_optional(&st.db)
        .await
        .ok()
        .flatten()
}

/// Inserts an (already-encrypted) DM and returns the client payload with the
/// decrypted content and both user objects included.
async fn create_dm(st: &AppState, sender_id: &str, receiver_id: &str, plain: &str) -> Option<Value> {
    let encrypted = st.dm_crypto.encrypt(plain);
    let id = new_id();
    let now = now_db();
    sqlx::query(
        "INSERT INTO DirectMessage (id, content, senderId, receiverId, createdAt) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&encrypted)
    .bind(sender_id)
    .bind(receiver_id)
    .bind(&now)
    .execute(&st.db)
    .await
    .ok()?;

    let dm: DirectMessage = sqlx::query_as("SELECT * FROM DirectMessage WHERE id = ?")
        .bind(&id)
        .fetch_one(&st.db)
        .await
        .ok()?;
    let sender = load_public_user(st, sender_id).await?;
    let receiver = load_public_user(st, receiver_id).await?;

    let mut payload = serde_json::to_value(&dm).unwrap();
    payload["content"] = json!(plain);
    payload["sender"] = serde_json::to_value(&sender).unwrap();
    payload["receiver"] = serde_json::to_value(&receiver).unwrap();
    Some(payload)
}

async fn ev_send_dm(st: AppState, socket: SocketRef, v: Value) {
    let Some(user_id) = uid(&socket) else { return };
    let Some(receiver_id) = str_field(&v, "receiverId") else { return };
    let content = str_field(&v, "content").unwrap_or_default();
    if content.trim().is_empty() {
        return;
    }
    if let Some(payload) = create_dm(&st, &user_id, &receiver_id, &content).await {
        st.emit_to(&receiver_id, "dm_received", payload.clone());
        st.emit_to(&user_id, "dm_received", payload);
    }
}

async fn ev_dm_call_dial(st: AppState, socket: SocketRef, v: Value) {
    let Some(user_id) = uid(&socket) else { return };
    let Some(receiver_id) = str_field(&v, "receiverId") else { return };
    let room_id = v.get("roomId").cloned().unwrap_or(Value::Null);
    let caller = v.get("caller").cloned().unwrap_or(Value::Null);

    let _ = socket
        .to(receiver_id.clone())
        .emit("dm_call_incoming", &json!({ "caller": caller, "roomId": room_id }));

    if let Some(payload) = create_dm(&st, &user_id, &receiver_id, "📞 Начал звонок").await {
        st.emit_to(&receiver_id, "dm_received", payload.clone());
        st.emit_to(&user_id, "dm_received", payload);
    }
}

async fn ev_dm_call_accept(_st: AppState, socket: SocketRef, v: Value) {
    let Some(caller_id) = str_field(&v, "callerId") else { return };
    let room_id = v.get("roomId").cloned().unwrap_or(Value::Null);
    let _ = socket
        .to(caller_id)
        .emit("dm_call_accepted", &json!({ "roomId": room_id }));
}

async fn ev_dm_call_decline(_st: AppState, socket: SocketRef, v: Value) {
    let Some(caller_id) = str_field(&v, "callerId") else { return };
    let _ = socket.to(caller_id).emit("dm_call_declined", &Value::Null);
}

async fn ev_dm_call_hangup(_st: AppState, socket: SocketRef, v: Value) {
    let Some(receiver_id) = str_field(&v, "receiverId") else { return };
    let _ = socket.to(receiver_id).emit("dm_call_hungup", &Value::Null);
}

async fn ev_update_status(st: AppState, socket: SocketRef, v: Value) {
    let Some(user_id) = uid(&socket) else { return };
    let Some(status) = str_field(&v, "status") else { return };
    if !["online", "idle", "dnd", "offline"].contains(&status.as_str()) {
        return;
    }

    if sqlx::query("UPDATE User SET status = ?, updatedAt = ? WHERE id = ?")
        .bind(&status)
        .bind(now_db())
        .bind(&user_id)
        .execute(&st.db)
        .await
        .is_err()
    {
        return;
    }

    st.emit_all("status_changed", json!({ "userId": user_id, "status": status }));

    // Reflect the new profile into any voice entry for this socket.
    let sid = socket.id.to_string();
    let patch: Option<VoiceProfile> = sqlx::query_as(
        "SELECT username, avatar, status, bio, pronouns, isBot FROM User WHERE id = ?",
    )
    .bind(&user_id)
    .fetch_optional(&st.db)
    .await
    .ok()
    .flatten();
    if let Some(p) = patch {
        let patch_val = json!({
            "username": p.username, "avatar": p.avatar, "status": p.status, "bio": p.bio
        });
        if st.voice.update_profile(&sid, &patch_val) {
            st.emit_all("voice_state_sync", st.voice.snapshot());
        }
    }
}

/// Rich Presence update from a client (the akami_rpc agent, or any client).
/// Payload: `{ rpcAppName, rpcDetails?, rpcState?, rpcStartTime?, rpcLargeImage? }`.
/// An empty/absent `rpcAppName` clears the presence.
async fn ev_update_presence(st: AppState, socket: SocketRef, v: Value) {
    let Some(user_id) = uid(&socket) else { return };

    let app_name = str_field(&v, "rpcAppName").filter(|s| !s.trim().is_empty());

    let payload = match app_name {
        None => {
            st.clear_presence(&user_id);
            AppState::cleared_presence(&user_id)
        }
        Some(app_name) => {
            let presence = json!({
                "rpcAppName": app_name,
                "rpcDetails": v.get("rpcDetails").cloned().unwrap_or(Value::Null),
                "rpcState": v.get("rpcState").cloned().unwrap_or(Value::Null),
                "rpcStartTime": v.get("rpcStartTime").cloned().unwrap_or(Value::Null),
                "rpcLargeImage": v.get("rpcLargeImage").cloned().unwrap_or(Value::Null),
            });
            st.set_presence(&user_id, presence.clone());
            let mut out = presence;
            out["userId"] = json!(user_id);
            out
        }
    };

    st.emit_all("presence_changed", payload);
}

async fn ev_disconnect(st: AppState, socket: SocketRef) {
    let user_id = uid(&socket);
    let sid = socket.id.to_string();

    // Notify voice rooms this socket was in.
    for channel_id in st.voice.channels_with_socket(&sid) {
        st.emit_to(
            &voice_room(&channel_id),
            "user_left_voice",
            json!({ "socketId": sid, "userId": user_id }),
        );
    }
    if st.voice.remove_socket(&sid) {
        st.emit_all("voice_state_sync", st.voice.snapshot());
    }

    if let Some(user_id) = user_id {
        let _ = sqlx::query("UPDATE User SET status = 'offline' WHERE id = ?")
            .bind(&user_id)
            .execute(&st.db)
            .await;
        // Presence dies with the connection.
        if st.clear_presence(&user_id) {
            st.emit_all("presence_changed", AppState::cleared_presence(&user_id));
        }
        st.emit_all("status_changed", json!({ "userId": user_id, "status": "offline" }));
    }
}
