use chrono::{DateTime, Utc};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use std::str::FromStr;
use uuid::Uuid;

pub type Db = SqlitePool;

/// Idempotent schema matching the final state of the Prisma migrations,
/// so a fresh deploy works while an existing Prisma dev.db is untouched.
const SCHEMA: &str = include_str!("schema.sql");

pub async fn connect(database_url: &str) -> Result<Db, sqlx::Error> {
    let options = SqliteConnectOptions::from_str(database_url)?
        .create_if_missing(true)
        .foreign_keys(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?;

    sqlx::raw_sql(SCHEMA).execute(&pool).await?;
    Ok(pool)
}

/// sqlx 0.9 only accepts `&'static str` SQL; queries built with `format!`
/// (e.g. embedding PUBLIC_USER_COLS) go through this explicit escape hatch.
/// Never interpolate user input — values always go through `.bind()`.
pub fn sql(query: String) -> sqlx::AssertSqlSafe<String> {
    sqlx::AssertSqlSafe(query)
}

pub fn new_id() -> String {
    Uuid::new_v4().to_string()
}

/// Timestamp in the exact TEXT format Prisma writes to SQLite:
/// `2026-07-18T15:36:18.886+00:00`.
pub fn now_db() -> String {
    to_db_date(Utc::now())
}

pub fn to_db_date(dt: DateTime<Utc>) -> String {
    dt.format("%Y-%m-%dT%H:%M:%S%.3f+00:00").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::*;

    /// Decode real Prisma-written rows from a copy of the original dev.db
    /// and check the JSON date format matches Prisma's output.
    #[tokio::test]
    async fn reads_prisma_database() {
        let src = "../akamiwtf/server/dev.db";
        if !std::path::Path::new(src).exists() {
            eprintln!("skipping: original dev.db not found");
            return;
        }
        let dst = std::env::temp_dir().join("akami_test_dev.db");
        std::fs::copy(src, &dst).unwrap();
        let url = format!("sqlite:{}", dst.display());

        let pool = connect(&url).await.unwrap();

        let users: Vec<User> = sqlx::query_as("SELECT * FROM User")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert!(!users.is_empty());

        let json = serde_json::to_value(&users[0]).unwrap();
        let created = json["createdAt"].as_str().unwrap();
        assert!(
            created.ends_with('Z') && created.len() == 24,
            "bad date format: {created}"
        );
        assert!(json.get("password").is_none(), "password must not serialize");
        assert!(json.get("displayName").is_some(), "must be camelCase");

        let _messages: Vec<Message> = sqlx::query_as("SELECT * FROM Message LIMIT 5")
            .fetch_all(&pool)
            .await
            .unwrap();
        let _dms: Vec<DirectMessage> = sqlx::query_as("SELECT * FROM DirectMessage LIMIT 5")
            .fetch_all(&pool)
            .await
            .unwrap();
        let _friends: Vec<Friendship> = sqlx::query_as("SELECT * FROM Friendship LIMIT 5")
            .fetch_all(&pool)
            .await
            .unwrap();
        let _servers: Vec<Server> = sqlx::query_as("SELECT * FROM Server LIMIT 5")
            .fetch_all(&pool)
            .await
            .unwrap();
        let _channels: Vec<Channel> = sqlx::query_as("SELECT * FROM Channel LIMIT 5")
            .fetch_all(&pool)
            .await
            .unwrap();

        let pub_users: Vec<PublicUser> =
            sqlx::query_as(sql(format!("SELECT {PUBLIC_USER_COLS} FROM User LIMIT 3")))
                .fetch_all(&pool)
                .await
                .unwrap();
        assert!(!pub_users.is_empty());

        // Round-trip: write a row with our own id/date helpers and read it back.
        let id = new_id();
        let now = now_db();
        sqlx::query(
            "INSERT INTO User (id, username, email, password, updatedAt, createdAt) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(format!("rust_test_{}", &id[..8]))
        .bind(format!("{id}@test.local"))
        .bind("x")
        .bind(&now)
        .bind(&now)
        .execute(&pool)
        .await
        .unwrap();

        let u: User = sqlx::query_as("SELECT * FROM User WHERE id = ?")
            .bind(&id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(u.status, "online");
        assert!(!u.is_bot);
    }
}
