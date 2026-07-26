//! Bearer-token auth extractor, mirroring server/src/middleware/auth.js.

use axum::extract::FromRequestParts;
use axum::http::{request::Parts, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::json;
use sqlx::FromRow;

use crate::auth;
use crate::models::prisma_date;
use crate::state::AppState;

/// The user shape Node's authMiddleware attaches to `req.user`
/// (includes email, no displayName).
#[derive(Debug, Clone, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
#[sqlx(rename_all = "camelCase")]
pub struct AuthedUser {
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

pub struct AuthError(pub &'static str);

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        (StatusCode::UNAUTHORIZED, Json(json!({ "error": self.0 }))).into_response()
    }
}

impl FromRequestParts<AppState> for AuthedUser {
    type Rejection = AuthError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let header = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        let Some(token) = header.strip_prefix("Bearer ") else {
            return Err(AuthError("Unauthorized, no token provided"));
        };

        let Some(user_id) = auth::verify(token, &state.config.jwt_secret) else {
            return Err(AuthError("Unauthorized, invalid token"));
        };

        let user: Option<AuthedUser> = sqlx::query_as(
            "SELECT id, username, displayName, email, avatar, banner, status, bio, pronouns, \
             profileColor, badges, customStatus, socials, createdAt FROM User WHERE id = ?",
        )
        .bind(&user_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|_| AuthError("Unauthorized, invalid token"))?;

        user.ok_or(AuthError("User not found"))
    }
}
