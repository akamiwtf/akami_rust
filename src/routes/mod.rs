pub mod auth;
pub mod dto;
pub mod invites;
pub mod servers;
pub mod upload;
pub mod users;

use axum::extract::Request;
use axum::http::{header, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;
use axum::routing::{delete, get, post, put};
use axum::Router;
use tower_http::services::ServeDir;

use crate::state::AppState;

/// Mirrors JS encodeURIComponent for the characters that occur in names.
pub fn encode_uri_component(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'!' | b'~' | b'*'
            | b'\'' | b'(' | b')' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Serves the uploads directory.
///
/// Anything that is not a picture, a clip or a sound is handed over as a download
/// rather than rendered: uploads are arbitrary files now, and an .html or .svg one
/// opened straight from this origin would run its own script. Media stays inline —
/// `<img>` and `<video>` load it regardless of the header, so the chat is unaffected.
pub fn uploads_router() -> Router<AppState> {
    Router::new()
        .fallback_service(ServeDir::new("uploads"))
        .layer(axum::middleware::from_fn(as_download))
}

async fn as_download(req: Request, next: Next) -> Response {
    let stored = req
        .uri()
        .path()
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string();

    let res = next.run(req).await;

    let inline = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| {
            // svg is an image that can carry script, so it is not in this group.
            (ct.starts_with("image/") && !ct.starts_with("image/svg"))
                || ct.starts_with("video/")
                || ct.starts_with("audio/")
        });
    if inline {
        return res;
    }

    let name = upload::pretty_name(&stored);
    let mut res = res;
    if let Ok(value) = HeaderValue::from_str(&format!(
        "attachment; filename*=UTF-8''{}",
        encode_uri_component(&name)
    )) {
        res.headers_mut().insert(header::CONTENT_DISPOSITION, value);
    }
    res.headers_mut()
        .insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    res
}

pub fn api_router() -> Router<AppState> {
    Router::new()
        // auth
        .route("/api/auth/register", post(auth::register))
        .route("/api/auth/login", post(auth::login))
        .route("/api/auth/bot/create", post(auth::bot_create))
        .route("/api/auth/me", get(auth::me))
        // upload
        .route("/api/upload", post(upload::upload))
        // servers
        .route("/api/servers", get(servers::list_servers).post(servers::create_server))
        .route("/api/servers/join", post(servers::join_server))
        .route(
            "/api/servers/{serverId}",
            put(servers::update_server).delete(servers::delete_server),
        )
        .route("/api/servers/{serverId}/leave", delete(servers::leave_server))
        // invites
        .route(
            "/api/servers/{serverId}/invites",
            get(invites::list_invites).post(invites::create_invite),
        )
        .route("/api/servers/invites/{inviteId}", delete(invites::revoke_invite))
        .route("/api/invites/{code}", get(invites::preview_invite))
        .route(
            "/api/servers/{serverId}/channels",
            get(servers::list_channels).post(servers::create_channel),
        )
        .route(
            "/api/servers/channels/{channelId}/messages",
            get(servers::channel_messages),
        )
        .route(
            "/api/servers/channels/{channelId}",
            delete(servers::delete_channel),
        )
        // users (mounted at /api in Node)
        .route("/api/servers/{serverId}/members", get(users::server_members))
        .route("/api/users", get(users::list_users))
        .route("/api/users/profile", put(users::update_profile))
        .route("/api/users/search", get(users::search_users))
        .route("/api/friends", get(users::list_friends))
        .route("/api/friends/request", post(users::friend_request))
        .route("/api/friends/accept/{friendshipId}", post(users::friend_accept))
        .route("/api/friends/{friendshipId}", delete(users::friend_remove))
        .route("/api/users/dms/{otherUserId}", get(users::user_dms))
        .route("/api/users/{userId}", get(users::get_user))
        .route("/api/users/{userId}/mutuals", get(users::user_mutuals))
        .route("/api/users/{userId}/badges", get(users::user_badges))
}
