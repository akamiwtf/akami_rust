mod auth;
mod config;
mod crypto;
mod db;
mod middleware;
mod models;
mod realtime;
mod routes;
mod state;
mod voice;

#[cfg(test)]
mod tests_integration;
#[cfg(test)]
mod tests_socket;

use std::fs;

use axum::{extract::DefaultBodyLimit, routing::get, Router};
use socketioxide::SocketIo;
use tower_http::{cors::CorsLayer, services::ServeDir};

use config::Config;

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt().with_target(false).init();

    let config = Config::from_env();

    if fs::metadata("uploads").is_err() {
        fs::create_dir("uploads").expect("failed to create uploads directory");
    }

    let pool = db::connect(&config.database_url)
        .await
        .expect("failed to connect to database");
    tracing::info!("Connected to database {}", config.database_url);

    let app_state = state::AppState::new(pool, config.clone());

    // Socket.io layer; must share the same AppState so REST handlers can emit.
    let (socket_layer, io) = SocketIo::builder()
        .with_state(app_state.clone())
        .build_layer();
    realtime::register(&io, &app_state);

    // Pure API server: the desktop app (akami_desktop) is the only client.
    let app = Router::new()
        .route("/api/health", get(health))
        .merge(routes::api_router())
        .nest_service("/uploads", ServeDir::new("uploads"))
        .layer(socket_layer)
        .layer(CorsLayer::permissive())
        .layer(DefaultBodyLimit::max(10 * 1024 * 1024))
        .with_state(app_state);

    let addr = format!("0.0.0.0:{}", config.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("failed to bind port");
    tracing::info!("Server is running on port {}", config.port);
    axum::serve(listener, app).await.expect("server error");
}

async fn health() -> &'static str {
    "Akami WTF Discord Clone API is running..."
}
