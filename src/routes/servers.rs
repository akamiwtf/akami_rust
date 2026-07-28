use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::dto::{e500, err, ApiResult, MessageUser, MESSAGE_USER_COLS};
use crate::db::{new_id, now_db, sql};
use crate::middleware::AuthedUser;
use crate::models::{Channel, Message, Server};
use crate::state::AppState;

fn invite_code() -> String {
    let mut buf = [0u8; 4];
    rand::rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

#[derive(Serialize)]
struct ServerWithChannels {
    #[serde(flatten)]
    server: Server,
    channels: Vec<Channel>,
}

async fn channels_of(st: &AppState, server_id: &str) -> Result<Vec<Channel>, super::dto::ApiError> {
    sqlx::query_as("SELECT * FROM Channel WHERE serverId = ?")
        .bind(server_id)
        .fetch_all(&st.db)
        .await
        .map_err(e500)
}

/// GET /api/servers — servers the user is a member of, each with channels.
pub async fn list_servers(State(st): State<AppState>, me: AuthedUser) -> ApiResult<Json<Value>> {
    let servers: Vec<Server> = sqlx::query_as(
        "SELECT s.* FROM Server s \
         JOIN ServerMember m ON m.serverId = s.id WHERE m.userId = ?",
    )
    .bind(&me.id)
    .fetch_all(&st.db)
    .await
    .map_err(e500)?;

    let mut out = Vec::with_capacity(servers.len());
    for server in servers {
        let channels = channels_of(&st, &server.id).await?;
        out.push(ServerWithChannels { server, channels });
    }
    Ok(Json(json!(out)))
}

#[derive(Deserialize)]
pub struct CreateServerBody {
    name: Option<String>,
}

pub async fn create_server(
    State(st): State<AppState>,
    me: AuthedUser,
    Json(body): Json<CreateServerBody>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let Some(name) = body.name.filter(|n| !n.is_empty()) else {
        return Err(err(StatusCode::BAD_REQUEST, "Server name is required"));
    };

    let server_id = new_id();
    let now = now_db();
    let image_url = format!(
        "https://api.dicebear.com/7.x/initials/svg?seed={}",
        super::encode_uri_component(&name)
    );

    let mut tx = st.db.begin().await.map_err(e500)?;
    sqlx::query(
        "INSERT INTO Server (id, name, inviteCode, ownerId, imageUrl, createdAt, updatedAt) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&server_id)
    .bind(&name)
    .bind(invite_code())
    .bind(&me.id)
    .bind(&image_url)
    .bind(&now)
    .bind(&now)
    .execute(&mut *tx)
    .await
    .map_err(e500)?;

    sqlx::query(
        "INSERT INTO ServerMember (id, role, userId, serverId, createdAt, updatedAt) \
         VALUES (?, 'ADMIN', ?, ?, ?, ?)",
    )
    .bind(new_id())
    .bind(&me.id)
    .bind(&server_id)
    .bind(&now)
    .bind(&now)
    .execute(&mut *tx)
    .await
    .map_err(e500)?;

    sqlx::query(
        "INSERT INTO Channel (id, name, type, serverId, createdAt, updatedAt) \
         VALUES (?, 'general', 'TEXT', ?, ?, ?)",
    )
    .bind(new_id())
    .bind(&server_id)
    .bind(&now)
    .bind(&now)
    .execute(&mut *tx)
    .await
    .map_err(e500)?;

    tx.commit().await.map_err(e500)?;

    let server: Server = sqlx::query_as("SELECT * FROM Server WHERE id = ?")
        .bind(&server_id)
        .fetch_one(&st.db)
        .await
        .map_err(e500)?;
    let channels = channels_of(&st, &server_id).await?;

    Ok((
        StatusCode::CREATED,
        Json(json!(ServerWithChannels { server, channels })),
    ))
}

#[derive(Deserialize)]
pub struct JoinBody {
    #[serde(rename = "inviteCode")]
    invite_code: Option<String>,
}

pub async fn join_server(
    State(st): State<AppState>,
    me: AuthedUser,
    Json(body): Json<JoinBody>,
) -> ApiResult<Json<Value>> {
    let Some(code) = body.invite_code.filter(|c| !c.is_empty()) else {
        return Err(err(StatusCode::BAD_REQUEST, "Invite code is required"));
    };

    // Either one of the server's invite links or its own legacy code; the resolver
    // also refuses a link that is revoked, expired or used up.
    let ((server_id, _, _), invite) = super::invites::resolve_code(&st, &code).await?;
    let server: Option<Server> = sqlx::query_as("SELECT * FROM Server WHERE id = ?")
        .bind(&server_id)
        .fetch_optional(&st.db)
        .await
        .map_err(e500)?;
    let Some(server) = server else {
        return Err(err(
            StatusCode::NOT_FOUND,
            "Server not found with this invite code",
        ));
    };

    let existing: Option<(String,)> =
        sqlx::query_as("SELECT id FROM ServerMember WHERE serverId = ? AND userId = ?")
            .bind(&server.id)
            .bind(&me.id)
            .fetch_optional(&st.db)
            .await
            .map_err(e500)?;
    if existing.is_some() {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "You are already a member of this server",
        ));
    }

    let now = now_db();
    sqlx::query(
        "INSERT INTO ServerMember (id, role, userId, serverId, createdAt, updatedAt) \
         VALUES (?, 'GUEST', ?, ?, ?, ?)",
    )
    .bind(new_id())
    .bind(&me.id)
    .bind(&server.id)
    .bind(&now)
    .bind(&now)
    .execute(&st.db)
    .await
    .map_err(e500)?;

    // Counted only now, once the member row exists: a failed join must not eat a
    // use of a one-off link.
    if let Some(invite) = invite {
        super::invites::count_use(&st, &invite.id).await;
    }

    let channels = channels_of(&st, &server.id).await?;
    Ok(Json(json!(ServerWithChannels { server, channels })))
}

async fn membership_role(st: &AppState, server_id: &str, user_id: &str) -> Option<String> {
    sqlx::query_as::<_, (String,)>(
        "SELECT role FROM ServerMember WHERE serverId = ? AND userId = ?",
    )
    .bind(server_id)
    .bind(user_id)
    .fetch_optional(&st.db)
    .await
    .ok()
    .flatten()
    .map(|r| r.0)
}

pub async fn list_channels(
    State(st): State<AppState>,
    me: AuthedUser,
    Path(server_id): Path<String>,
) -> ApiResult<Json<Value>> {
    if membership_role(&st, &server_id, &me.id).await.is_none() {
        return Err(err(
            StatusCode::FORBIDDEN,
            "Access denied. You are not a member of this server",
        ));
    }
    let channels = channels_of(&st, &server_id).await?;
    Ok(Json(json!(channels)))
}

#[derive(Deserialize)]
pub struct CreateChannelBody {
    name: Option<String>,
    #[serde(rename = "type")]
    channel_type: Option<String>,
}

pub async fn create_channel(
    State(st): State<AppState>,
    me: AuthedUser,
    Path(server_id): Path<String>,
    Json(body): Json<CreateChannelBody>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let Some(name) = body.name else {
        return Err(err(StatusCode::BAD_REQUEST, "Channel name is required"));
    };
    if name.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Channel name is required"));
    }

    match membership_role(&st, &server_id, &me.id).await.as_deref() {
        Some("ADMIN") | Some("MODERATOR") => {}
        _ => {
            return Err(err(
                StatusCode::FORBIDDEN,
                "Access denied. Insufficient permissions",
            ))
        }
    }

    let id = new_id();
    let now = now_db();
    let ctype = body.channel_type.filter(|t| !t.is_empty()).unwrap_or_else(|| "TEXT".into());
    sqlx::query(
        "INSERT INTO Channel (id, name, type, serverId, createdAt, updatedAt) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(name.trim())
    .bind(&ctype)
    .bind(&server_id)
    .bind(&now)
    .bind(&now)
    .execute(&st.db)
    .await
    .map_err(e500)?;

    let channel: Channel = sqlx::query_as("SELECT * FROM Channel WHERE id = ?")
        .bind(&id)
        .fetch_one(&st.db)
        .await
        .map_err(e500)?;
    Ok((StatusCode::CREATED, Json(json!(channel))))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MessageResponse {
    #[serde(flatten)]
    message: MessageOut,
    user: MessageUser,
    /// The quoted message, when this one is a reply and the original still exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    reply_to: Option<Value>,
    /// Grouped by emoji; an empty array when nobody reacted.
    reactions: Value,
    /// Who wrote the message this was forwarded from, when it was.
    #[serde(skip_serializing_if = "Option::is_none")]
    forwarded_from_user: Option<Value>,
}

/// Message with `components` decoded from its JSON string into an array/null.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MessageOut {
    id: String,
    content: String,
    components: Option<Value>,
    user_id: String,
    channel_id: String,
    reply_to_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(with = "crate::models::prisma_date_opt")]
    pinned_at: Option<chrono::DateTime<chrono::Utc>>,
    forwarded_from: Option<String>,
    #[serde(with = "crate::models::prisma_date")]
    created_at: chrono::DateTime<chrono::Utc>,
    #[serde(with = "crate::models::prisma_date")]
    updated_at: chrono::DateTime<chrono::Utc>,
}

fn decode_components(raw: Option<String>) -> Option<Value> {
    raw.and_then(|s| serde_json::from_str::<Value>(&s).ok())
}

pub async fn channel_messages(
    State(st): State<AppState>,
    me: AuthedUser,
    Path(channel_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let channel: Option<Channel> = sqlx::query_as("SELECT * FROM Channel WHERE id = ?")
        .bind(&channel_id)
        .fetch_optional(&st.db)
        .await
        .map_err(e500)?;
    let Some(channel) = channel else {
        return Err(err(StatusCode::NOT_FOUND, "Channel not found"));
    };

    if membership_role(&st, &channel.server_id, &me.id).await.is_none() {
        return Err(err(StatusCode::FORBIDDEN, "Access denied"));
    }

    let messages: Vec<Message> =
        sqlx::query_as("SELECT * FROM Message WHERE channelId = ? ORDER BY createdAt ASC")
            .bind(&channel_id)
            .fetch_all(&st.db)
            .await
            .map_err(e500)?;

    let mut out = Vec::with_capacity(messages.len());
    for m in messages {
        let user: MessageUser =
            sqlx::query_as(sql(format!("SELECT {MESSAGE_USER_COLS} FROM User WHERE id = ?")))
                .bind(&m.user_id)
                .fetch_one(&st.db)
                .await
                .map_err(e500)?;
        let reply_to = match &m.reply_to_id {
            Some(rid) => crate::messages::reply_preview(&st, rid, false).await,
            None => None,
        };
        let reactions = crate::messages::reactions_for(&st, &m.id).await;
        let forwarded_from_user = match &m.forwarded_from {
            Some(author) => Some(crate::messages::forwarded_label(&st, author).await),
            None => None,
        };
        out.push(MessageResponse {
            message: MessageOut {
                id: m.id,
                content: m.content,
                components: decode_components(m.components),
                user_id: m.user_id,
                channel_id: m.channel_id,
                reply_to_id: m.reply_to_id,
                pinned_at: m.pinned_at,
                forwarded_from: m.forwarded_from.clone(),
                created_at: m.created_at,
                updated_at: m.updated_at,
            },
            user,
            reply_to,
            reactions,
            forwarded_from_user,
        });
    }
    Ok(Json(json!(out)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerUpdate {
    name: Option<String>,
    /// Missing keeps the current value, `null` clears it, a string sets it.
    #[serde(default, deserialize_with = "super::dto::double_option_pub")]
    image_url: Option<Option<String>>,
    #[serde(default, deserialize_with = "super::dto::double_option_pub")]
    banner_url: Option<Option<String>>,
}

/// Renames a server and sets its picture and banner. Owner only — the same rule
/// the client shows the settings button by.
pub async fn update_server(
    State(st): State<AppState>,
    me: AuthedUser,
    Path(server_id): Path<String>,
    Json(body): Json<ServerUpdate>,
) -> ApiResult<Json<Value>> {
    let owner: Option<(String,)> = sqlx::query_as("SELECT ownerId FROM Server WHERE id = ?")
        .bind(&server_id)
        .fetch_optional(&st.db)
        .await
        .map_err(e500)?;
    let Some((owner_id,)) = owner else {
        return Err(err(StatusCode::NOT_FOUND, "Server not found"));
    };
    if owner_id != me.id {
        return Err(err(StatusCode::FORBIDDEN, "Only the owner can do that"));
    }

    let name = body.name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());

    let mut qb: sqlx::QueryBuilder<sqlx::Sqlite> =
        sqlx::QueryBuilder::new(r#"UPDATE "Server" SET "updatedAt" = "#);
    qb.push_bind(crate::db::now_db());
    if let Some(name) = &name {
        qb.push(r#", "name" = "#).push_bind(name.clone());
    }
    if let Some(image) = &body.image_url {
        qb.push(r#", "imageUrl" = "#).push_bind(image.clone());
    }
    if let Some(banner) = &body.banner_url {
        qb.push(r#", "bannerUrl" = "#).push_bind(banner.clone());
    }
    qb.push(" WHERE id = ").push_bind(server_id.clone());
    qb.build().execute(&st.db).await.map_err(e500)?;

    let server: Server = sqlx::query_as("SELECT * FROM Server WHERE id = ?")
        .bind(&server_id)
        .fetch_one(&st.db)
        .await
        .map_err(e500)?;

    // Everyone's sidebar shows this, so everyone hears about it.
    st.emit_all("server_updated", serde_json::to_value(&server).unwrap());
    Ok(Json(json!(server)))
}

/// Leaving a server. The owner cannot: their server would be left without one, so
/// they delete it instead.
pub async fn leave_server(
    State(st): State<AppState>,
    me: AuthedUser,
    Path(server_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let owner: Option<(String,)> = sqlx::query_as("SELECT ownerId FROM Server WHERE id = ?")
        .bind(&server_id)
        .fetch_optional(&st.db)
        .await
        .map_err(e500)?;
    let Some((owner_id,)) = owner else {
        return Err(err(StatusCode::NOT_FOUND, "Server not found"));
    };
    if owner_id == me.id {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "Владелец не может выйти — сервер можно удалить",
        ));
    }

    let deleted = sqlx::query("DELETE FROM ServerMember WHERE serverId = ? AND userId = ?")
        .bind(&server_id)
        .bind(&me.id)
        .execute(&st.db)
        .await
        .map_err(e500)?;
    if deleted.rows_affected() == 0 {
        return Err(err(StatusCode::BAD_REQUEST, "You are not a member"));
    }

    // The member list in everyone's sidebar shows this.
    st.emit_all(
        "server_member_left",
        json!({ "serverId": server_id, "userId": me.id }),
    );
    Ok(Json(json!({ "serverId": server_id, "left": true })))
}

/// Deleting a server, owner only. Channels, members and invites go with it through
/// the foreign keys; reactions are keyed by message id alone, so they are cleared by
/// hand first.
pub async fn delete_server(
    State(st): State<AppState>,
    me: AuthedUser,
    Path(server_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let owner: Option<(String,)> = sqlx::query_as("SELECT ownerId FROM Server WHERE id = ?")
        .bind(&server_id)
        .fetch_optional(&st.db)
        .await
        .map_err(e500)?;
    let Some((owner_id,)) = owner else {
        return Err(err(StatusCode::NOT_FOUND, "Server not found"));
    };
    if owner_id != me.id {
        return Err(err(StatusCode::FORBIDDEN, "Only the owner can do that"));
    }

    sqlx::query(
        "DELETE FROM Reaction WHERE messageId IN \
         (SELECT m.id FROM Message m JOIN Channel c ON c.id = m.channelId WHERE c.serverId = ?)",
    )
    .bind(&server_id)
    .execute(&st.db)
    .await
    .map_err(e500)?;

    sqlx::query("DELETE FROM Server WHERE id = ?")
        .bind(&server_id)
        .execute(&st.db)
        .await
        .map_err(e500)?;

    st.emit_all("server_deleted", json!({ "serverId": server_id }));
    Ok(Json(json!({ "serverId": server_id, "deleted": true })))
}

pub async fn delete_channel(
    State(st): State<AppState>,
    me: AuthedUser,
    Path(channel_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let channel: Option<Channel> = sqlx::query_as("SELECT * FROM Channel WHERE id = ?")
        .bind(&channel_id)
        .fetch_optional(&st.db)
        .await
        .map_err(e500)?;
    let Some(channel) = channel else {
        return Err(err(StatusCode::NOT_FOUND, "Channel not found"));
    };

    match membership_role(&st, &channel.server_id, &me.id).await.as_deref() {
        Some("ADMIN") | Some("MODERATOR") => {}
        _ => {
            return Err(err(
                StatusCode::FORBIDDEN,
                "Access denied. Insufficient permissions",
            ))
        }
    }

    sqlx::query("DELETE FROM Channel WHERE id = ?")
        .bind(&channel_id)
        .execute(&st.db)
        .await
        .map_err(e500)?;

    st.emit_all("channel_deleted", json!({ "channelId": channel_id }));

    Ok(Json(json!({ "message": "Channel deleted successfully" })))
}
