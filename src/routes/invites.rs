//! Invite links to a server.
//!
//! A link is a short code plus the rules for using it: how many people may join
//! through it and until when. Creating and listing them belongs to the owner;
//! using one is open to anyone with the code.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::dto::{e500, err, ApiResult, DmUser, DM_USER_COLS};
use crate::db::{new_id, now_db, sql, to_db_date};
use crate::middleware::AuthedUser;
use crate::models::Invite;
use crate::state::AppState;

/// Codes are read aloud and typed, so the alphabet leaves out the characters that
/// look alike (0/O, 1/l/I).
const CODE_ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
const CODE_LEN: usize = 8;

fn new_code() -> String {
    use rand::Rng;
    let mut bytes = [0u8; CODE_LEN];
    rand::rng().fill_bytes(&mut bytes);
    // Modulo bias over 31 letters is irrelevant here: the code only has to be hard
    // to guess among the codes that exist, and it is 8 characters of it.
    bytes
        .iter()
        .map(|b| CODE_ALPHABET[*b as usize % CODE_ALPHABET.len()] as char)
        .collect()
}

/// Why an invite cannot be used, if it cannot.
fn unusable_reason(invite: &Invite) -> Option<&'static str> {
    if invite.revoked_at.is_some() {
        return Some("revoked");
    }
    if invite.expires_at.is_some_and(|at| at <= Utc::now()) {
        return Some("expired");
    }
    if invite.max_uses.is_some_and(|max| invite.uses >= max) {
        return Some("used up");
    }
    None
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InviteOut {
    #[serde(flatten)]
    invite: Invite,
    /// Who made it, for the list in the settings.
    creator: Option<DmUser>,
    /// `true` while the link still works — the client shows the state rather than
    /// working the rules out again.
    active: bool,
}

async fn load_creator(st: &AppState, id: &str) -> Option<DmUser> {
    sqlx::query_as(sql(format!("SELECT {DM_USER_COLS} FROM User WHERE id = ?")))
        .bind(id)
        .fetch_optional(&st.db)
        .await
        .ok()
        .flatten()
}

async fn owner_only(st: &AppState, server_id: &str, user_id: &str) -> ApiResult<()> {
    let owner: Option<(String,)> = sqlx::query_as("SELECT ownerId FROM Server WHERE id = ?")
        .bind(server_id)
        .fetch_optional(&st.db)
        .await
        .map_err(e500)?;
    let Some((owner_id,)) = owner else {
        return Err(err(StatusCode::NOT_FOUND, "Server not found"));
    };
    if owner_id != user_id {
        return Err(err(StatusCode::FORBIDDEN, "Only the owner can do that"));
    }
    Ok(())
}

pub async fn list_invites(
    State(st): State<AppState>,
    me: AuthedUser,
    Path(server_id): Path<String>,
) -> ApiResult<Json<Value>> {
    owner_only(&st, &server_id, &me.id).await?;

    let invites: Vec<Invite> =
        sqlx::query_as("SELECT * FROM Invite WHERE serverId = ? ORDER BY createdAt DESC")
            .bind(&server_id)
            .fetch_all(&st.db)
            .await
            .map_err(e500)?;

    let mut out = Vec::with_capacity(invites.len());
    for invite in invites {
        let creator = load_creator(&st, &invite.creator_id).await;
        let active = unusable_reason(&invite).is_none();
        out.push(InviteOut {
            invite,
            creator,
            active,
        });
    }
    Ok(Json(json!(out)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateInviteBody {
    /// How many people may join with it; omit or null for no limit.
    max_uses: Option<i64>,
    /// How long it lasts, in seconds; omit or null for forever.
    expires_in_seconds: Option<i64>,
}

pub async fn create_invite(
    State(st): State<AppState>,
    me: AuthedUser,
    Path(server_id): Path<String>,
    Json(body): Json<CreateInviteBody>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    owner_only(&st, &server_id, &me.id).await?;

    // A limit of zero or less is not a limit, it is a mistake; treat it as absent
    // rather than creating a link nobody can use.
    let max_uses = body.max_uses.filter(|n| *n > 0);
    let expires_at = body
        .expires_in_seconds
        .filter(|s| *s > 0)
        .map(|s| to_db_date(Utc::now() + Duration::seconds(s)));

    let id = new_id();
    // A collision is vanishingly unlikely, but a unique index would turn one into
    // a 500; a couple of retries cost nothing.
    let mut last_err = None;
    for _ in 0..5 {
        let code = new_code();
        let res = sqlx::query(
            "INSERT INTO Invite (id, code, serverId, creatorId, maxUses, uses, expiresAt, createdAt) \
             VALUES (?, ?, ?, ?, ?, 0, ?, ?)",
        )
        .bind(&id)
        .bind(&code)
        .bind(&server_id)
        .bind(&me.id)
        .bind(max_uses)
        .bind(&expires_at)
        .bind(now_db())
        .execute(&st.db)
        .await;
        match res {
            Ok(_) => {
                let invite: Invite = sqlx::query_as("SELECT * FROM Invite WHERE id = ?")
                    .bind(&id)
                    .fetch_one(&st.db)
                    .await
                    .map_err(e500)?;
                let creator = load_creator(&st, &invite.creator_id).await;
                return Ok((
                    StatusCode::CREATED,
                    Json(json!(InviteOut {
                        invite,
                        creator,
                        active: true
                    })),
                ));
            }
            Err(e) => last_err = Some(e),
        }
    }
    Err(e500(last_err.map(|e| e.to_string()).unwrap_or_default()))
}

pub async fn revoke_invite(
    State(st): State<AppState>,
    me: AuthedUser,
    Path(invite_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let invite: Option<Invite> = sqlx::query_as("SELECT * FROM Invite WHERE id = ?")
        .bind(&invite_id)
        .fetch_optional(&st.db)
        .await
        .map_err(e500)?;
    let Some(invite) = invite else {
        return Err(err(StatusCode::NOT_FOUND, "Invite not found"));
    };
    owner_only(&st, &invite.server_id, &me.id).await?;

    sqlx::query("UPDATE Invite SET revokedAt = ? WHERE id = ?")
        .bind(now_db())
        .bind(&invite_id)
        .execute(&st.db)
        .await
        .map_err(e500)?;

    Ok(Json(json!({ "id": invite_id, "revoked": true })))
}

/// What a code leads to, for the screen that asks "join this server?".
///
/// Deliberately thin: the name and picture of the server, and whether the code
/// still works. No member list, no channels — the caller is not in yet.
pub async fn preview_invite(
    State(st): State<AppState>,
    me: AuthedUser,
    Path(code): Path<String>,
) -> ApiResult<Json<Value>> {
    let (server, invite) = resolve_code(&st, &code).await?;

    let already: Option<(String,)> =
        sqlx::query_as("SELECT id FROM ServerMember WHERE serverId = ? AND userId = ?")
            .bind(&server.0)
            .bind(&me.id)
            .fetch_optional(&st.db)
            .await
            .map_err(e500)?;

    Ok(Json(json!({
        "code": code,
        "serverId": server.0,
        "serverName": server.1,
        "serverImageUrl": server.2,
        "alreadyMember": already.is_some(),
        // Absent for a legacy `Server.inviteCode` link, which has no rules.
        "invite": invite,
    })))
}

/// Finds the server a code points at, whether it is one of the new invites or the
/// server's own legacy code. Returns `(id, name, imageUrl)` and the invite, if any.
pub async fn resolve_code(
    st: &AppState,
    code: &str,
) -> ApiResult<((String, String, Option<String>), Option<Invite>)> {
    let invite: Option<Invite> = sqlx::query_as("SELECT * FROM Invite WHERE code = ?")
        .bind(code)
        .fetch_optional(&st.db)
        .await
        .map_err(e500)?;

    if let Some(invite) = invite {
        if let Some(reason) = unusable_reason(&invite) {
            return Err(err(
                StatusCode::GONE,
                match reason {
                    "revoked" => "Эта ссылка отключена",
                    "expired" => "Срок действия ссылки истёк",
                    _ => "Ссылка исчерпала лимит использований",
                },
            ));
        }
        let server: Option<(String, String, Option<String>)> =
            sqlx::query_as("SELECT id, name, imageUrl FROM Server WHERE id = ?")
                .bind(&invite.server_id)
                .fetch_optional(&st.db)
                .await
                .map_err(e500)?;
        let Some(server) = server else {
            return Err(err(StatusCode::NOT_FOUND, "Server not found"));
        };
        return Ok((server, Some(invite)));
    }

    let server: Option<(String, String, Option<String>)> =
        sqlx::query_as("SELECT id, name, imageUrl FROM Server WHERE inviteCode = ?")
            .bind(code)
            .fetch_optional(&st.db)
            .await
            .map_err(e500)?;
    match server {
        Some(server) => Ok((server, None)),
        None => Err(err(StatusCode::NOT_FOUND, "Приглашение не найдено")),
    }
}

/// Counts one use against an invite. Called once the member row exists.
pub async fn count_use(st: &AppState, invite_id: &str) {
    let _ = sqlx::query("UPDATE Invite SET uses = uses + 1 WHERE id = ?")
        .bind(invite_id)
        .execute(&st.db)
        .await;
}
