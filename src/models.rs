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

/// Same format for a column that may be NULL.
pub mod prisma_date_opt {
    use super::*;
    use serde::Serializer;

    pub fn serialize<S: Serializer>(dt: &Option<DateTime<Utc>>, s: S) -> Result<S::Ok, S::Error> {
        match dt {
            Some(dt) => super::prisma_date::serialize(dt, s),
            None => s.serialize_none(),
        }
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
    pub profile_color: String,
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
    pub profile_color: String,
    pub badges: String,
    pub custom_status: String,
    pub socials: String,
    #[serde(with = "prisma_date")]
    pub created_at: DateTime<Utc>,
}

/// Column list matching `PublicUser`, for SELECT statements.
pub const PUBLIC_USER_COLS: &str = "id, username, displayName, avatar, banner, status, bio, \
    pronouns, profileColor, badges, customStatus, socials, createdAt";

#[derive(Debug, Clone, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
#[sqlx(rename_all = "camelCase")]
pub struct Server {
    pub id: String,
    pub name: String,
    pub image_url: Option<String>,
    /// Wide image shown behind the server's name in the channel list.
    pub banner_url: Option<String>,
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
    /// The message this one answers, if any.
    pub reply_to_id: Option<String>,
    /// When it was pinned; absent while it is not.
    #[serde(with = "prisma_date_opt", skip_serializing_if = "Option::is_none")]
    pub pinned_at: Option<DateTime<Utc>>,
    /// Author of the message this one was forwarded from, if it was.
    pub forwarded_from: Option<String>,
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
    /// Set the first time the message is edited; absent otherwise.
    #[serde(with = "prisma_date_opt", skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
    /// The message this one answers, if any.
    pub reply_to_id: Option<String>,
    /// When it was pinned; absent while it is not.
    #[serde(with = "prisma_date_opt", skip_serializing_if = "Option::is_none")]
    pub pinned_at: Option<DateTime<Utc>>,
    /// Author of the message this one was forwarded from, if it was.
    pub forwarded_from: Option<String>,
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

/// An invite link to a server.
#[derive(Debug, Clone, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
#[sqlx(rename_all = "camelCase")]
pub struct Invite {
    pub id: String,
    pub code: String,
    pub server_id: String,
    pub creator_id: String,
    /// How many people may use it in total; `None` for no limit.
    pub max_uses: Option<i64>,
    pub uses: i64,
    /// When it stops working; `None` for never.
    #[serde(with = "prisma_date_opt", skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    /// Set when it was switched off by hand.
    #[serde(with = "prisma_date_opt", skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<DateTime<Utc>>,
    #[serde(with = "prisma_date")]
    pub created_at: DateTime<Utc>,
}
