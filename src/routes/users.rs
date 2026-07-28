use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::QueryBuilder;

use super::dto::{
    e500, err, ApiResult, ApiUser, DmUser, ProfileUpdate, ProfileUser, API_USER_COLS, DM_USER_COLS,
    PROFILE_USER_COLS,
};
use crate::db::{sql, to_db_date};
use crate::middleware::AuthedUser;
use crate::models::{DirectMessage, Friendship};
use crate::state::AppState;

/// GET /api/servers/:serverId/members
pub async fn server_members(
    State(st): State<AppState>,
    me: AuthedUser,
    Path(server_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let is_member: Option<(String,)> =
        sqlx::query_as("SELECT id FROM ServerMember WHERE serverId = ? AND userId = ?")
            .bind(&server_id)
            .bind(&me.id)
            .fetch_optional(&st.db)
            .await
            .map_err(e500)?;
    if is_member.is_none() {
        return Err(err(StatusCode::FORBIDDEN, "Access denied"));
    }

    let rows: Vec<(ApiUser, String)> = {
        let members: Vec<(String, String)> =
            sqlx::query_as("SELECT userId, role FROM ServerMember WHERE serverId = ?")
                .bind(&server_id)
                .fetch_all(&st.db)
                .await
                .map_err(e500)?;
        let mut out = Vec::with_capacity(members.len());
        for (user_id, role) in members {
            let user: ApiUser =
                sqlx::query_as(sql(format!("SELECT {API_USER_COLS} FROM User WHERE id = ?")))
                    .bind(&user_id)
                    .fetch_one(&st.db)
                    .await
                    .map_err(e500)?;
            out.push((user, role));
        }
        out
    };

    let users: Vec<Value> = rows
        .into_iter()
        .map(|(user, role)| {
            let mut v = serde_json::to_value(user).unwrap();
            v["role"] = json!(role);
            st.apply_presence(&mut v);
            v
        })
        .collect();
    Ok(Json(json!(users)))
}

/// GET /api/users — everyone except the caller.
pub async fn list_users(State(st): State<AppState>, me: AuthedUser) -> ApiResult<Json<Value>> {
    let users: Vec<ApiUser> =
        sqlx::query_as(sql(format!("SELECT {API_USER_COLS} FROM User WHERE id != ?")))
            .bind(&me.id)
            .fetch_all(&st.db)
            .await
            .map_err(e500)?;
    let users: Vec<Value> = users
        .into_iter()
        .map(|u| {
            let mut v = serde_json::to_value(u).unwrap();
            st.apply_presence(&mut v);
            v
        })
        .collect();
    Ok(Json(json!(users)))
}

/// PUT /api/users/profile
pub async fn update_profile(
    State(st): State<AppState>,
    me: AuthedUser,
    Json(body): Json<ProfileUpdate>,
) -> ApiResult<Json<Value>> {
    let mut qb: QueryBuilder<sqlx::Sqlite> = QueryBuilder::new(r#"UPDATE "User" SET "updatedAt" = "#);
    qb.push_bind(to_db_date(Utc::now()));

    // `|| undefined`: only a truthy (non-empty) value overwrites.
    if let Some(Some(v)) = body.username.filter(|o| o.as_deref().is_some_and(|s| !s.is_empty())) {
        qb.push(r#", "username" = "#).push_bind(v);
    }
    if let Some(Some(v)) = body.status.filter(|o| o.as_deref().is_some_and(|s| !s.is_empty())) {
        qb.push(r#", "status" = "#).push_bind(v);
    }

    // `!== undefined`: present overwrites (null clears), missing keeps.
    macro_rules! set_nullable {
        ($opt:expr, $col:literal) => {
            if let Some(inner) = $opt {
                qb.push(concat!(r#", ""#, $col, r#"" = "#)).push_bind(inner);
            }
        };
    }
    set_nullable!(body.display_name, "displayName");
    set_nullable!(body.avatar, "avatar");
    set_nullable!(body.banner, "banner");
    set_nullable!(body.bio, "bio");
    set_nullable!(body.pronouns, "pronouns");
    set_nullable!(body.profile_color, "profileColor");
    set_nullable!(body.badges, "badges");
    set_nullable!(body.custom_status, "customStatus");
    set_nullable!(body.socials, "socials");

    qb.push(r#" WHERE id = "#).push_bind(me.id.clone());
    qb.build().execute(&st.db).await.map_err(e500)?;

    let user: ProfileUser =
        sqlx::query_as(sql(format!("SELECT {PROFILE_USER_COLS} FROM User WHERE id = ?")))
            .bind(&me.id)
            .fetch_one(&st.db)
            .await
            .map_err(e500)?;
    Ok(Json(json!(user)))
}

// --- Friend system ---

#[derive(Serialize)]
struct FriendshipResponse {
    #[serde(flatten)]
    friendship: Friendship,
    sender: ApiUser,
    receiver: ApiUser,
}

async fn load_api_user(st: &AppState, id: &str) -> Result<ApiUser, super::dto::ApiError> {
    sqlx::query_as(sql(format!("SELECT {API_USER_COLS} FROM User WHERE id = ?")))
        .bind(id)
        .fetch_one(&st.db)
        .await
        .map_err(e500)
}

async fn friendship_response(
    st: &AppState,
    f: Friendship,
) -> Result<FriendshipResponse, super::dto::ApiError> {
    let sender = load_api_user(st, &f.sender_id).await?;
    let receiver = load_api_user(st, &f.receiver_id).await?;
    Ok(FriendshipResponse {
        friendship: f,
        sender,
        receiver,
    })
}

#[derive(Deserialize)]
pub struct FriendRequestBody {
    username: Option<String>,
}

pub async fn friend_request(
    State(st): State<AppState>,
    me: AuthedUser,
    Json(body): Json<FriendRequestBody>,
) -> ApiResult<Json<Value>> {
    let Some(username) = body.username.filter(|u| !u.is_empty()) else {
        return Err(err(StatusCode::BAD_REQUEST, "Username is required"));
    };

    let target: Option<(String,)> = sqlx::query_as("SELECT id FROM User WHERE username = ?")
        .bind(&username)
        .fetch_optional(&st.db)
        .await
        .map_err(e500)?;
    let Some((target_id,)) = target else {
        return Err(err(StatusCode::NOT_FOUND, "Пользователь не найден"));
    };
    if target_id == me.id {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "Нельзя добавить себя в друзья",
        ));
    }

    let existing: Option<Friendship> = sqlx::query_as(
        "SELECT * FROM Friendship WHERE (senderId = ? AND receiverId = ?) \
         OR (senderId = ? AND receiverId = ?)",
    )
    .bind(&me.id)
    .bind(&target_id)
    .bind(&target_id)
    .bind(&me.id)
    .fetch_optional(&st.db)
    .await
    .map_err(e500)?;
    if let Some(existing) = existing {
        if existing.status == "ACCEPTED" {
            return Err(err(StatusCode::BAD_REQUEST, "Вы уже друзья"));
        }
        return Err(err(StatusCode::BAD_REQUEST, "Заявка уже отправлена"));
    }

    let id = crate::db::new_id();
    let now = crate::db::now_db();
    sqlx::query(
        "INSERT INTO Friendship (id, status, senderId, receiverId, createdAt, updatedAt) \
         VALUES (?, 'PENDING', ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&me.id)
    .bind(&target_id)
    .bind(&now)
    .bind(&now)
    .execute(&st.db)
    .await
    .map_err(e500)?;

    let f: Friendship = sqlx::query_as("SELECT * FROM Friendship WHERE id = ?")
        .bind(&id)
        .fetch_one(&st.db)
        .await
        .map_err(e500)?;
    let resp = friendship_response(&st, f).await?;
    let payload = serde_json::to_value(&resp).unwrap();
    st.emit_to(&target_id, "friend_request_received", payload.clone());
    st.emit_to(&me.id, "friend_request_sent", payload.clone());
    Ok(Json(payload))
}

pub async fn friend_accept(
    State(st): State<AppState>,
    me: AuthedUser,
    Path(friendship_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let f: Option<Friendship> = sqlx::query_as("SELECT * FROM Friendship WHERE id = ?")
        .bind(&friendship_id)
        .fetch_optional(&st.db)
        .await
        .map_err(e500)?;
    let Some(f) = f else {
        return Err(err(StatusCode::NOT_FOUND, "Request not found"));
    };
    if f.receiver_id != me.id {
        return Err(err(StatusCode::FORBIDDEN, "Access denied"));
    }
    if f.status == "ACCEPTED" {
        return Err(err(StatusCode::BAD_REQUEST, "Already accepted"));
    }

    sqlx::query("UPDATE Friendship SET status = 'ACCEPTED', updatedAt = ? WHERE id = ?")
        .bind(crate::db::now_db())
        .bind(&f.id)
        .execute(&st.db)
        .await
        .map_err(e500)?;

    let updated: Friendship = sqlx::query_as("SELECT * FROM Friendship WHERE id = ?")
        .bind(&f.id)
        .fetch_one(&st.db)
        .await
        .map_err(e500)?;
    let (sender_id, receiver_id) = (updated.sender_id.clone(), updated.receiver_id.clone());
    let resp = friendship_response(&st, updated).await?;
    let payload = serde_json::to_value(&resp).unwrap();
    st.emit_to(&sender_id, "friend_request_accepted", payload.clone());
    st.emit_to(&receiver_id, "friend_request_accepted", payload.clone());
    Ok(Json(payload))
}

pub async fn friend_remove(
    State(st): State<AppState>,
    me: AuthedUser,
    Path(friendship_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let f: Option<Friendship> = sqlx::query_as("SELECT * FROM Friendship WHERE id = ?")
        .bind(&friendship_id)
        .fetch_optional(&st.db)
        .await
        .map_err(e500)?;
    let Some(f) = f else {
        return Err(err(StatusCode::NOT_FOUND, "Not found"));
    };
    if f.sender_id != me.id && f.receiver_id != me.id {
        return Err(err(StatusCode::FORBIDDEN, "Access denied"));
    }

    sqlx::query("DELETE FROM Friendship WHERE id = ?")
        .bind(&f.id)
        .execute(&st.db)
        .await
        .map_err(e500)?;

    let payload = json!({ "friendshipId": f.id });
    st.emit_to(&f.sender_id, "friend_removed", payload.clone());
    st.emit_to(&f.receiver_id, "friend_removed", payload);
    Ok(Json(json!({ "success": true })))
}

pub async fn list_friends(State(st): State<AppState>, me: AuthedUser) -> ApiResult<Json<Value>> {
    let friendships: Vec<Friendship> = sqlx::query_as(
        "SELECT * FROM Friendship WHERE senderId = ? OR receiverId = ? ORDER BY createdAt DESC",
    )
    .bind(&me.id)
    .bind(&me.id)
    .fetch_all(&st.db)
    .await
    .map_err(e500)?;

    let mut out = Vec::with_capacity(friendships.len());
    for f in friendships {
        out.push(friendship_response(&st, f).await?);
    }
    Ok(Json(json!(out)))
}

#[derive(Deserialize)]
pub struct SearchQuery {
    q: Option<String>,
}

pub async fn search_users(
    State(st): State<AppState>,
    me: AuthedUser,
    Query(query): Query<SearchQuery>,
) -> ApiResult<Json<Value>> {
    let Some(q) = query.q.filter(|q| !q.is_empty()) else {
        return Ok(Json(json!([])));
    };
    let users: Vec<ApiUser> = sqlx::query_as(sql(format!(
        "SELECT {API_USER_COLS} FROM User WHERE username LIKE ? AND id != ? LIMIT 20"
    )))
    .bind(format!("%{q}%"))
    .bind(&me.id)
    .fetch_all(&st.db)
    .await
    .map_err(e500)?;
    Ok(Json(json!(users)))
}

pub async fn get_user(
    State(st): State<AppState>,
    _me: AuthedUser,
    Path(user_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let user: Option<ApiUser> =
        sqlx::query_as(sql(format!("SELECT {API_USER_COLS} FROM User WHERE id = ?")))
            .bind(&user_id)
            .fetch_optional(&st.db)
            .await
            .map_err(e500)?;
    match user {
        Some(u) => {
            let mut v = serde_json::to_value(u).unwrap();
            st.apply_presence(&mut v);
            Ok(Json(v))
        }
        None => Err(err(StatusCode::NOT_FOUND, "User not found")),
    }
}

pub async fn user_mutuals(
    State(st): State<AppState>,
    me: AuthedUser,
    Path(user_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let my_server_ids: Vec<(String,)> =
        sqlx::query_as("SELECT serverId FROM ServerMember WHERE userId = ?")
            .bind(&me.id)
            .fetch_all(&st.db)
            .await
            .map_err(e500)?;
    let my_server_ids: Vec<String> = my_server_ids.into_iter().map(|r| r.0).collect();

    let mut mutual_servers = Vec::new();
    for sid in &my_server_ids {
        let is_member: Option<(String,)> =
            sqlx::query_as("SELECT id FROM ServerMember WHERE userId = ? AND serverId = ?")
                .bind(&user_id)
                .bind(sid)
                .fetch_optional(&st.db)
                .await
                .map_err(e500)?;
        if is_member.is_some() {
            let srv: Option<(String, String, Option<String>)> =
                sqlx::query_as("SELECT id, name, imageUrl FROM Server WHERE id = ?")
                    .bind(sid)
                    .fetch_optional(&st.db)
                    .await
                    .map_err(e500)?;
            if let Some((id, name, image_url)) = srv {
                mutual_servers.push(json!({ "id": id, "name": name, "imageUrl": image_url }));
            }
        }
    }

    let my_friend_ids = accepted_friend_ids(&st, &me.id).await?;
    let their_friend_ids = accepted_friend_ids(&st, &user_id).await?;
    let mutual_ids: Vec<String> = my_friend_ids
        .into_iter()
        .filter(|id| their_friend_ids.contains(id))
        .collect();

    let mut mutual_friends = Vec::new();
    for id in mutual_ids {
        let u: Option<(String, String, Option<String>, Option<String>, String)> = sqlx::query_as(
            "SELECT id, username, displayName, avatar, status FROM User WHERE id = ?",
        )
        .bind(&id)
        .fetch_optional(&st.db)
        .await
        .map_err(e500)?;
        if let Some((id, username, display_name, avatar, status)) = u {
            mutual_friends.push(json!({
                "id": id, "username": username, "displayName": display_name,
                "avatar": avatar, "status": status
            }));
        }
    }

    Ok(Json(json!({
        "mutualServers": mutual_servers,
        "mutualFriends": mutual_friends
    })))
}

async fn accepted_friend_ids(
    st: &AppState,
    user_id: &str,
) -> Result<Vec<String>, super::dto::ApiError> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT senderId, receiverId FROM Friendship \
         WHERE status = 'ACCEPTED' AND (senderId = ? OR receiverId = ?)",
    )
    .bind(user_id)
    .bind(user_id)
    .fetch_all(&st.db)
    .await
    .map_err(e500)?;
    Ok(rows
        .into_iter()
        .map(|(s, r)| if s == user_id { r } else { s })
        .collect())
}

pub async fn user_badges(
    State(st): State<AppState>,
    _me: AuthedUser,
    Path(user_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let user: Option<(String, bool, chrono::DateTime<Utc>)> =
        sqlx::query_as("SELECT id, isBot, createdAt FROM User WHERE id = ?")
            .bind(&user_id)
            .fetch_optional(&st.db)
            .await
            .map_err(e500)?;
    let Some((_, is_bot, created_at)) = user else {
        return Err(err(StatusCode::NOT_FOUND, "User not found"));
    };

    let mut badges = Vec::new();

    let owned: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM Server WHERE ownerId = ?")
        .bind(&user_id)
        .fetch_one(&st.db)
        .await
        .map_err(e500)?;
    if owned.0 > 0 {
        badges.push(json!({ "id": "server_owner", "label": "Владелец сервера", "icon": "shield" }));
    }
    if is_bot {
        badges.push(json!({ "id": "bot", "label": "Бот", "icon": "smart_toy" }));
    }
    let days = (Utc::now() - created_at).num_milliseconds() as f64 / (1000.0 * 60.0 * 60.0 * 24.0);
    if days < 7.0 {
        badges.push(json!({ "id": "newcomer", "label": "Новичок", "icon": "auto_awesome" }));
    }
    let friend_count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM Friendship WHERE status = 'ACCEPTED' AND (senderId = ? OR receiverId = ?)",
    )
    .bind(&user_id)
    .bind(&user_id)
    .fetch_one(&st.db)
    .await
    .map_err(e500)?;
    if friend_count.0 >= 5 {
        badges.push(json!({ "id": "social", "label": "Общительный", "icon": "group" }));
    }
    let msg_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM Message WHERE userId = ?")
        .bind(&user_id)
        .fetch_one(&st.db)
        .await
        .map_err(e500)?;
    if msg_count.0 >= 50 {
        badges.push(json!({ "id": "active", "label": "Активный", "icon": "local_fire_department" }));
    }

    Ok(Json(json!(badges)))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DmResponse {
    #[serde(flatten)]
    dm: DirectMessage,
    sender: DmUser,
    receiver: DmUser,
    #[serde(skip_serializing_if = "Option::is_none")]
    reply_to: Option<Value>,
    reactions: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    forwarded_from_user: Option<Value>,
}

#[derive(Deserialize)]
pub struct DmQuery {
    limit: Option<i64>,
    before: Option<String>,
}

pub async fn user_dms(
    State(st): State<AppState>,
    me: AuthedUser,
    Path(other_user_id): Path<String>,
    Query(query): Query<DmQuery>,
) -> ApiResult<Json<Value>> {
    let limit = query.limit.unwrap_or(100);

    let mut qb: QueryBuilder<sqlx::Sqlite> = QueryBuilder::new(
        "SELECT * FROM DirectMessage WHERE ((senderId = ",
    );
    qb.push_bind(me.id.clone())
        .push(" AND receiverId = ")
        .push_bind(other_user_id.clone())
        .push(") OR (senderId = ")
        .push_bind(other_user_id.clone())
        .push(" AND receiverId = ")
        .push_bind(me.id.clone())
        .push("))");

    if let Some(before) = query.before.as_deref() {
        // Normalize any RFC3339 form (…Z or …+00:00) to the DB's stored format
        // so the lexicographic `<` compare stays chronological.
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(before) {
            let db_date = to_db_date(dt.with_timezone(&Utc));
            qb.push(" AND createdAt < ").push_bind(db_date);
        }
    }

    qb.push(" ORDER BY createdAt DESC LIMIT ").push_bind(limit);

    let mut dms: Vec<DirectMessage> = qb
        .build_query_as()
        .fetch_all(&st.db)
        .await
        .map_err(e500)?;

    // Ascending chronological order for the client.
    dms.reverse();

    let mut out = Vec::with_capacity(dms.len());
    for mut dm in dms {
        dm.content = st.dm_crypto.decrypt(&dm.content);
        let sender = load_dm_user(&st, &dm.sender_id).await?;
        let receiver = load_dm_user(&st, &dm.receiver_id).await?;
        let reply_to = match &dm.reply_to_id {
            Some(rid) => crate::messages::reply_preview(&st, rid, true).await,
            None => None,
        };
        let reactions = crate::messages::reactions_for(&st, &dm.id).await;
        let forwarded_from_user = match &dm.forwarded_from {
            Some(author) => Some(crate::messages::forwarded_label(&st, author).await),
            None => None,
        };
        out.push(DmResponse {
            dm,
            sender,
            receiver,
            reply_to,
            reactions,
            forwarded_from_user,
        });
    }
    Ok(Json(json!(out)))
}

async fn load_dm_user(st: &AppState, id: &str) -> Result<DmUser, super::dto::ApiError> {
    sqlx::query_as(sql(format!("SELECT {DM_USER_COLS} FROM User WHERE id = ?")))
        .bind(id)
        .fetch_one(&st.db)
        .await
        .map_err(e500)
}
