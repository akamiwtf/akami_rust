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
    add_missing_columns(&pool).await?;
    Ok(pool)
}

/// Columns added after the first release. `CREATE TABLE IF NOT EXISTS` leaves
/// existing databases alone, so each one is added here if absent — `ADD COLUMN`
/// errors when the column is already there, hence the pragma check.
const ADDED_COLUMNS: &[(&str, &str, &str)] = &[
    (
        "User",
        "profileColor",
        r#"ALTER TABLE "User" ADD COLUMN "profileColor" TEXT NOT NULL DEFAULT ''"#,
    ),
    (
        "DirectMessage",
        "updatedAt",
        r#"ALTER TABLE "DirectMessage" ADD COLUMN "updatedAt" DATETIME"#,
    ),
    (
        "Message",
        "replyToId",
        r#"ALTER TABLE "Message" ADD COLUMN "replyToId" TEXT"#,
    ),
    (
        "DirectMessage",
        "replyToId",
        r#"ALTER TABLE "DirectMessage" ADD COLUMN "replyToId" TEXT"#,
    ),
    (
        "Message",
        "pinnedAt",
        r#"ALTER TABLE "Message" ADD COLUMN "pinnedAt" DATETIME"#,
    ),
    (
        "DirectMessage",
        "pinnedAt",
        r#"ALTER TABLE "DirectMessage" ADD COLUMN "pinnedAt" DATETIME"#,
    ),
    (
        "Message",
        "forwardedFrom",
        r#"ALTER TABLE "Message" ADD COLUMN "forwardedFrom" TEXT"#,
    ),
    (
        "DirectMessage",
        "forwardedFrom",
        r#"ALTER TABLE "DirectMessage" ADD COLUMN "forwardedFrom" TEXT"#,
    ),
    (
        "Server",
        "bannerUrl",
        r#"ALTER TABLE "Server" ADD COLUMN "bannerUrl" TEXT"#,
    ),
];

async fn add_missing_columns(pool: &Db) -> Result<(), sqlx::Error> {
    for (table, column, ddl) in ADDED_COLUMNS {
        let exists: Option<(i64,)> = sqlx::query_as(sql(format!(
            "SELECT 1 FROM pragma_table_info('{table}') WHERE name = ?"
        )))
        .bind(column)
        .fetch_optional(pool)
        .await?;
        if exists.is_none() {
            sqlx::raw_sql(sqlx::AssertSqlSafe(*ddl)).execute(pool).await?;
        }
    }
    Ok(())
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

    /// A database created before `profileColor` existed must gain the column on
    /// connect — `CREATE TABLE IF NOT EXISTS` alone would leave it behind.
    #[tokio::test]
    async fn adds_columns_to_an_older_database() {
        let path = std::env::temp_dir().join(format!("akami_migr_{}.db", new_id()));
        let url = format!("sqlite:{}", path.display());

        // Stand up a User table without the newer column, as an old deploy has.
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::from_str(&url)
                    .unwrap()
                    .create_if_missing(true),
            )
            .await
            .unwrap();
        sqlx::raw_sql(
            r#"CREATE TABLE "User" ("id" TEXT NOT NULL PRIMARY KEY, "username" TEXT NOT NULL,
               "email" TEXT NOT NULL, "password" TEXT NOT NULL, "updatedAt" DATETIME NOT NULL)"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(r#"INSERT INTO "User" VALUES (?, 'old', 'o@x.com', 'pw', '2026-01-01')"#)
            .bind(new_id())
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;

        let pool = connect(&url).await.unwrap();
        // Present, and the existing row got the default rather than NULL.
        let (color,): (String,) = sqlx::query_as(r#"SELECT "profileColor" FROM "User""#)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(color, "");

        // Connecting again must not fail on the already-added column.
        pool.close().await;
        connect(&url).await.unwrap();
        let _ = std::fs::remove_file(&path);
    }

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
