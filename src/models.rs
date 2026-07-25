use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::FromRow;

/// Prisma serializes DateTime to JSON as `2026-07-18T15:36:18.886Z` —
/// exactly three fraction digits and a literal `Z`.
pub mod prisma_date {
    use super::*;
    use serde::Serializer;

    pub fn serialize<S: Serializer>(dt: &DateTime<Utc>, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
    }
}

/// Full user row, including private columns. `password` is never serialized.
#[derive(Debug, Clone, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
#[sqlx(rename_all = "camelCase")]
pub struct User {
    pub id: String,
    pub username: String,
    pub display_name: Option<String>,
    pub email: String,
    #[serde(skip_serializing)]
    pub password: String,
    pub avatar: Option<String>,
    pub banner: Option<String>,
    pub status: String,
    pub bio: String,
    pub pronouns: String,
    pub badges: String,
    pub custom_status: String,
    pub socials: String,
    pub is_bot: bool,
    pub owner_id: Option<String>,
    #[serde(with = "prisma_date")]
    pub created_at: DateTime<Utc>,
    #[serde(with = "prisma_date")]
    pub updated_at: DateTime<Utc>,
}

/// Public user shape embedded in direct-message payloads.
#[derive(Debug, Clone, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
#[sqlx(rename_all = "camelCase")]
pub struct PublicUser {
    pub id: String,
    pub username: String,
    pub display_name: Option<String>,
    pub avatar: Option<String>,
    pub banner: Option<String>,
    pub status: String,
    pub bio: String,
    pub pronouns: String,
    pub badges: String,
    pub custom_status: String,
    pub socials: String,
    #[serde(with = "prisma_date")]
    pub created_at: DateTime<Utc>,
}

/// Column list matching `PublicUser`, for SELECT statements.
pub const PUBLIC_USER_COLS: &str = "id, username, displayName, avatar, banner, status, bio, \
    pronouns, badges, customStatus, socials, createdAt";

#[derive(Debug, Clone, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
#[sqlx(rename_all = "camelCase")]
pub struct Server {
    pub id: String,
    pub name: String,
    pub image_url: Option<String>,
    pub invite_code: String,
    pub owner_id: String,
    #[serde(with = "prisma_date")]
    pub created_at: DateTime<Utc>,
    #[serde(with = "prisma_date")]
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
#[sqlx(rename_all = "camelCase")]
pub struct Channel {
    pub id: String,
    pub name: String,
    #[sqlx(rename = "type")]
    #[serde(rename = "type")]
    pub channel_type: String,
    pub server_id: String,
    #[serde(with = "prisma_date")]
    pub created_at: DateTime<Utc>,
    #[serde(with = "prisma_date")]
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
#[sqlx(rename_all = "camelCase")]
pub struct Message {
    pub id: String,
    pub content: String,
    pub components: Option<String>,
    pub user_id: String,
    pub channel_id: String,
    #[serde(with = "prisma_date")]
    pub created_at: DateTime<Utc>,
    #[serde(with = "prisma_date")]
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
#[sqlx(rename_all = "camelCase")]
pub struct DirectMessage {
    pub id: String,
    pub content: String,
    pub sender_id: String,
    pub receiver_id: String,
    #[serde(with = "prisma_date")]
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
#[sqlx(rename_all = "camelCase")]
pub struct Friendship {
    pub id: String,
    pub status: String,
    pub sender_id: String,
    pub receiver_id: String,
    #[serde(with = "prisma_date")]
    pub created_at: DateTime<Utc>,
    #[serde(with = "prisma_date")]
    pub updated_at: DateTime<Utc>,
}
