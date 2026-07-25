use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use rand::Rng;
use serde::Deserialize;
use serde_json::{json, Value};

use super::dto::{e500, err, ApiResult};
use crate::auth::{self, BOT_TOKEN_DAYS, USER_TOKEN_DAYS};
use crate::db::{new_id, now_db};
use crate::middleware::AuthedUser;
use crate::models::User;
use crate::state::AppState;

fn dicebear(seed: &str) -> String {
    format!(
        "https://api.dicebear.com/7.x/bottts/svg?seed={}",
        urlencoding(seed)
    )
}

/// Matches JS encodeURIComponent for the characters that appear in usernames.
fn urlencoding(s: &str) -> String {
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

fn hex_bytes(n: usize) -> String {
    let mut buf = vec![0u8; n];
    rand::rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

#[derive(Deserialize)]
pub struct RegisterBody {
    username: Option<String>,
    email: Option<String>,
    password: Option<String>,
}

pub async fn register(
    State(st): State<AppState>,
    Json(body): Json<RegisterBody>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let (Some(username), Some(email), Some(password)) = (body.username, body.email, body.password)
    else {
        return Err(err(StatusCode::BAD_REQUEST, "All fields are required"));
    };
    if username.is_empty() || email.is_empty() || password.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "All fields are required"));
    }

    let existing: Option<(String,)> =
        sqlx::query_as("SELECT id FROM User WHERE email = ? OR username = ?")
            .bind(&email)
            .bind(&username)
            .fetch_optional(&st.db)
            .await
            .map_err(e500)?;
    if existing.is_some() {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "Username or email already exists",
        ));
    }

    let hashed = bcrypt::hash(&password, 10).map_err(e500)?;
    let id = new_id();
    let now = now_db();
    let avatar = dicebear(&username);

    sqlx::query(
        "INSERT INTO User (id, username, email, password, avatar, createdAt, updatedAt) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&username)
    .bind(&email)
    .bind(&hashed)
    .bind(&avatar)
    .bind(&now)
    .bind(&now)
    .execute(&st.db)
    .await
    .map_err(e500)?;

    let token = auth::sign(&id, &st.config.jwt_secret, USER_TOKEN_DAYS);
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "token": token,
            "user": { "id": id, "username": username, "displayName": Value::Null, "email": email, "avatar": avatar }
        })),
    ))
}

#[derive(Deserialize)]
pub struct LoginBody {
    email: Option<String>,
    password: Option<String>,
}

pub async fn login(
    State(st): State<AppState>,
    Json(body): Json<LoginBody>,
) -> ApiResult<Json<Value>> {
    let (Some(email), Some(password)) = (body.email, body.password) else {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "Email and password are required",
        ));
    };
    if email.is_empty() || password.is_empty() {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "Email and password are required",
        ));
    }

    let user: Option<User> = sqlx::query_as("SELECT * FROM User WHERE email = ?")
        .bind(&email)
        .fetch_optional(&st.db)
        .await
        .map_err(e500)?;

    let Some(user) = user else {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid credentials"));
    };
    if user.is_bot {
        return Err(err(
            StatusCode::FORBIDDEN,
            "Cannot log in to a bot account directly.",
        ));
    }
    if !bcrypt::verify(&password, &user.password).unwrap_or(false) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid credentials"));
    }

    let token = auth::sign(&user.id, &st.config.jwt_secret, USER_TOKEN_DAYS);
    Ok(Json(json!({
        "token": token,
        "user": {
            "id": user.id, "username": user.username, "displayName": user.display_name,
            "email": user.email, "avatar": user.avatar
        }
    })))
}

#[derive(Deserialize)]
pub struct BotCreateBody {
    username: Option<String>,
}

pub async fn bot_create(
    State(st): State<AppState>,
    me: AuthedUser,
    Json(body): Json<BotCreateBody>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let Some(username) = body.username.filter(|u| !u.is_empty()) else {
        return Err(err(StatusCode::BAD_REQUEST, "Bot username is required"));
    };

    let existing: Option<(String,)> = sqlx::query_as("SELECT id FROM User WHERE username = ?")
        .bind(&username)
        .fetch_optional(&st.db)
        .await
        .map_err(e500)?;
    if existing.is_some() {
        return Err(err(StatusCode::BAD_REQUEST, "Username already exists"));
    }

    let email = format!("bot_{}@akami.local", hex_bytes(4));
    let temp_password = hex_bytes(16);
    let hashed = bcrypt::hash(&temp_password, 10).map_err(e500)?;
    let id = new_id();
    let now = now_db();
    let avatar = dicebear(&username);

    sqlx::query(
        "INSERT INTO User (id, username, email, password, avatar, isBot, ownerId, createdAt, updatedAt) \
         VALUES (?, ?, ?, ?, ?, 1, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&username)
    .bind(&email)
    .bind(&hashed)
    .bind(&avatar)
    .bind(&me.id)
    .bind(&now)
    .bind(&now)
    .execute(&st.db)
    .await
    .map_err(e500)?;

    let token = auth::sign(&id, &st.config.jwt_secret, BOT_TOKEN_DAYS);
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "token": token,
            "bot": { "id": id, "username": username, "email": email, "avatar": avatar }
        })),
    ))
}

pub async fn me(me: AuthedUser) -> Json<Value> {
    Json(json!({ "user": me }))
}
