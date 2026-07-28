//! Pieces a message carries besides its own text: what it answers, and what
//! people reacted to it with.
//!
//! Both the REST history and the socket events need these, and both kinds of
//! message (channel and direct) are handled here — reactions live in one table
//! keyed only by message id, so the lookup has to try each table in turn.

use serde_json::{json, Value};

use crate::db::{new_id, now_db, sql};
use crate::models::{DirectMessage, Message};
use crate::state::AppState;

/// Where a message lives, which decides who hears about a change to it.
pub enum MessageKind {
    /// Channel message: everyone in the channel.
    Channel { channel_id: String },
    /// Direct message: the two people in the conversation.
    Direct {
        sender_id: String,
        receiver_id: String,
    },
}

/// Finds a message by id in either table.
pub async fn locate(st: &AppState, message_id: &str) -> Option<MessageKind> {
    let channel: Option<Message> = sqlx::query_as("SELECT * FROM Message WHERE id = ?")
        .bind(message_id)
        .fetch_optional(&st.db)
        .await
        .ok()
        .flatten();
    if let Some(m) = channel {
        return Some(MessageKind::Channel {
            channel_id: m.channel_id,
        });
    }

    let dm: Option<DirectMessage> = sqlx::query_as("SELECT * FROM DirectMessage WHERE id = ?")
        .bind(message_id)
        .fetch_optional(&st.db)
        .await
        .ok()
        .flatten();
    dm.map(|dm| MessageKind::Direct {
        sender_id: dm.sender_id,
        receiver_id: dm.receiver_id,
    })
}

/// Reactions on a message, grouped by emoji: `[{ emoji, users: [userId, …] }]`.
///
/// The count and "did I react" are left to the client, which knows who it is —
/// the same payload then serves everyone.
pub async fn reactions_for(st: &AppState, message_id: &str) -> Value {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT emoji, userId FROM Reaction WHERE messageId = ? ORDER BY createdAt ASC",
    )
    .bind(message_id)
    .fetch_all(&st.db)
    .await
    .unwrap_or_default();

    // Insertion order, so the chips do not jump around between refreshes.
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    for (emoji, user_id) in rows {
        match groups.iter_mut().find(|(e, _)| *e == emoji) {
            Some((_, users)) => users.push(user_id),
            None => groups.push((emoji, vec![user_id])),
        }
    }

    json!(groups
        .into_iter()
        .map(|(emoji, users)| json!({ "emoji": emoji, "users": users }))
        .collect::<Vec<_>>())
}

/// Sets one person's reaction to a message, or takes it away when it is the one
/// they already had. Returns the message's reactions afterwards, or `None` if there
/// is no such message.
///
/// One reaction each: choosing a different emoji replaces the previous one rather
/// than adding to it.
pub async fn toggle_reaction(
    st: &AppState,
    message_id: &str,
    user_id: &str,
    emoji: &str,
) -> Option<Value> {
    let existing: Option<(String,)> =
        sqlx::query_as("SELECT emoji FROM Reaction WHERE messageId = ? AND userId = ?")
            .bind(message_id)
            .bind(user_id)
            .fetch_optional(&st.db)
            .await
            .ok()
            .flatten();

    // Whatever they had goes either way: the same emoji means they are taking it
    // back, a different one means they are changing their mind.
    if existing.is_some() {
        sqlx::query("DELETE FROM Reaction WHERE messageId = ? AND userId = ?")
            .bind(message_id)
            .bind(user_id)
            .execute(&st.db)
            .await
            .ok()?;
    }

    let same_again = existing.map(|(had,)| had == emoji).unwrap_or(false);
    if !same_again {
        sqlx::query(
            "INSERT INTO Reaction (id, messageId, userId, emoji, createdAt) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(new_id())
        .bind(message_id)
        .bind(user_id)
        .bind(emoji)
        .bind(now_db())
        .execute(&st.db)
        .await
        .ok()?;
    }

    Some(reactions_for(st, message_id).await)
}

/// Drops every reaction on a message. Called when the message itself goes: the
/// table has no foreign key to hang a cascade on.
pub async fn clear_reactions(st: &AppState, message_id: &str) {
    let _ = sqlx::query("DELETE FROM Reaction WHERE messageId = ?")
        .bind(message_id)
        .execute(&st.db)
        .await;
}

/// The quoted line shown above a reply: `{ id, userId, username, content }`.
///
/// `None` when the answered message is gone — a reply outlives what it answers.
pub async fn reply_preview(st: &AppState, reply_to_id: &str, direct: bool) -> Option<Value> {
    let (id, author_id, content) = if direct {
        let dm: DirectMessage = sqlx::query_as("SELECT * FROM DirectMessage WHERE id = ?")
            .bind(reply_to_id)
            .fetch_optional(&st.db)
            .await
            .ok()
            .flatten()?;
        // Stored encrypted; the preview travels in plain text like the message
        // payloads themselves do.
        let plain = st.dm_crypto.decrypt(&dm.content);
        (dm.id, dm.sender_id, plain)
    } else {
        let m: Message = sqlx::query_as("SELECT * FROM Message WHERE id = ?")
            .bind(reply_to_id)
            .fetch_optional(&st.db)
            .await
            .ok()
            .flatten()?;
        (m.id, m.user_id, m.content)
    };

    let username: Option<(String,)> =
        sqlx::query_as(sql("SELECT username FROM User WHERE id = ?".to_string()))
            .bind(&author_id)
            .fetch_optional(&st.db)
            .await
            .ok()
            .flatten();

    Some(json!({
        "id": id,
        "userId": author_id,
        "username": username.map(|(u,)| u),
        "content": content,
    }))
}

/// Who wrote the message a forward came from: `{ userId, username }`.
///
/// The client sends the source message's id rather than a name, so the label
/// cannot be invented — an id that matches nothing yields `None`.
pub async fn forwarded_author(st: &AppState, source_id: &str) -> Option<String> {
    let channel: Option<(String,)> = sqlx::query_as("SELECT userId FROM Message WHERE id = ?")
        .bind(source_id)
        .fetch_optional(&st.db)
        .await
        .ok()
        .flatten();
    if let Some((user_id,)) = channel {
        return Some(user_id);
    }
    let dm: Option<(String,)> = sqlx::query_as("SELECT senderId FROM DirectMessage WHERE id = ?")
        .bind(source_id)
        .fetch_optional(&st.db)
        .await
        .ok()
        .flatten();
    dm.map(|(user_id,)| user_id)
}

/// The label for a forwarded message: `{ userId, username }`.
pub async fn forwarded_label(st: &AppState, user_id: &str) -> Value {
    let username: Option<(String,)> =
        sqlx::query_as(sql("SELECT username FROM User WHERE id = ?".to_string()))
            .bind(user_id)
            .fetch_optional(&st.db)
            .await
            .ok()
            .flatten();
    json!({ "userId": user_id, "username": username.map(|(u,)| u) })
}
