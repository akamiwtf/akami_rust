//! Response/request shapes and column lists matching the Prisma `select`s
//! used in the original Express routes, so JSON stays byte-compatible.

use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;

use crate::models::prisma_date;

pub type ApiError = (StatusCode, Json<Value>);
pub type ApiResult<T> = Result<T, ApiError>;

pub fn err(status: StatusCode, msg: &str) -> ApiError {
    (status, Json(json!({ "error": msg })))
}

pub fn e500(_: impl std::fmt::Display) -> ApiError {
    err(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
}

/// friendUserSelect / server-members / users-list / single-user shape.
pub const API_USER_COLS: &str = "id, username, displayName, avatar, banner, status, bio, \
    pronouns, profileColor, isBot, badges, customStatus, socials, createdAt";

#[derive(Debug, Clone, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
#[sqlx(rename_all = "camelCase")]
pub struct ApiUser {
    pub id: String,
    pub username: String,
    pub display_name: Option<String>,
    pub avatar: Option<String>,
    pub banner: Option<String>,
    pub status: String,
    pub bio: String,
    pub pronouns: String,
    pub profile_color: String,
    pub is_bot: bool,
    pub badges: String,
    pub custom_status: String,
    pub socials: String,
    #[serde(with = "prisma_date")]
    pub created_at: DateTime<Utc>,
}

/// Message-author select in `GET /channels/:id/messages`.
pub const MESSAGE_USER_COLS: &str =
    "id, username, avatar, status, bio, createdAt, badges, customStatus, socials";

#[derive(Debug, Clone, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
#[sqlx(rename_all = "camelCase")]
pub struct MessageUser {
    pub id: String,
    pub username: String,
    pub avatar: Option<String>,
    pub status: String,
    pub bio: String,
    #[serde(with = "prisma_date")]
    pub created_at: DateTime<Utc>,
    pub badges: String,
    pub custom_status: String,
    pub socials: String,
}

/// Minimal user shape included with DMs.
pub const DM_USER_COLS: &str = "id, username, avatar";

#[derive(Debug, Clone, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
#[sqlx(rename_all = "camelCase")]
pub struct DmUser {
    pub id: String,
    pub username: String,
    pub avatar: Option<String>,
}

/// The profile-update return select (has email AND displayName).
#[derive(Debug, Clone, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
#[sqlx(rename_all = "camelCase")]
pub struct ProfileUser {
    pub id: String,
    pub username: String,
    pub display_name: Option<String>,
    pub email: String,
    pub avatar: Option<String>,
    pub banner: Option<String>,
    pub status: String,
    pub bio: String,
    pub pronouns: String,
    pub profile_color: String,
    pub badges: String,
    pub custom_status: String,
    pub socials: String,
    #[serde(with = "prisma_date")]
    pub created_at: DateTime<Utc>,
}

pub const PROFILE_USER_COLS: &str = "id, username, displayName, email, avatar, banner, status, \
    bio, pronouns, profileColor, badges, customStatus, socials, createdAt";

/// Deserializes into the JS three-state: a plain `Option<Option<T>>` collapses
/// a missing field and an explicit `null` both to `None`; wrapping the parsed
/// value in `Some` keeps them apart (missing → `None` via `default`, `null` →
/// `Some(None)`, value → `Some(Some(v))`).
fn double_option<'de, D, T>(de: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Deserialize::deserialize(de).map(Some)
}

/// Field missing = keep, explicit null = clear, value = set.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileUpdate {
    #[serde(default, deserialize_with = "double_option")]
    pub username: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub display_name: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub avatar: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub banner: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub status: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub bio: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub pronouns: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub profile_color: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub badges: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub custom_status: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub socials: Option<Option<String>>,
}
