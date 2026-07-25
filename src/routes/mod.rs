pub mod auth;
pub mod dto;
pub mod servers;
pub mod upload;
pub mod users;

use axum::routing::{delete, get, post, put};
use axum::Router;

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
