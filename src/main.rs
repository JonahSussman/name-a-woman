mod config;
mod db;
mod game;
mod handlers;
mod models;
mod normalize;
mod session;
mod wikidata;

use std::sync::Arc;

use axum::Router;
use axum::routing::get;
use tokio::sync::Mutex;
use tower_http::services::ServeDir;
use tracing_subscriber::EnvFilter;

pub struct AppState {
    pub db: Mutex<rusqlite::Connection>,
    pub config: config::Config,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let config = config::Config::from_env();
    tracing::info!(
        max_fallback_lookups = config.max_fallback_lookups,
        inactivity_timeout_secs = config.inactivity_timeout_secs,
        "loaded config"
    );

    let conn = db::init_db("data/names.db").expect("failed to initialize database");
    let state = Arc::new(AppState {
        db: Mutex::new(conn),
        config,
    });

    let api = Router::new()
        .route("/game/ws", get(game::ws_handler))
        .route("/leaderboard", get(handlers::leaderboard))
        .route("/stats/game/{id}", get(handlers::game_stats))
        .route("/health", get(handlers::health));

    let app = Router::new()
        .nest("/api", api)
        .fallback_service(ServeDir::new("static"))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await.unwrap();
    tracing::info!("listening on {}", listener.local_addr().unwrap());
    axum::serve(listener, app).await.unwrap();
}
