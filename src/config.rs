use std::env;

#[derive(Clone, Debug)]
pub struct Config {
    pub port: u16,
    pub jwt_secret: String,
    pub dm_encryption_key: String,
    pub database_url: String,
}

const DEFAULT_SECRET: &str = "akami-wtf-secret-key-12345";

impl Config {
    pub fn from_env() -> Self {
        let jwt_secret =
            env::var("JWT_SECRET").unwrap_or_else(|_| DEFAULT_SECRET.to_string());
        Self {
            port: env::var("PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(5000),
            dm_encryption_key: env::var("DM_ENCRYPTION_KEY")
                .unwrap_or_else(|_| jwt_secret.clone()),
            jwt_secret,
            database_url: env::var("DATABASE_URL")
                .unwrap_or_else(|_| "sqlite:dev.db".to_string()),
        }
    }
}
