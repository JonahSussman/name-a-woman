use std::sync::Arc;

use axum::Router;
use axum::routing::get;
use tokio::sync::Mutex;
use tower_http::services::ServeDir;
use tracing_subscriber::EnvFilter;

use name_a_woman::{AppState, config, db, game, handlers, wikidata};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let config = config::Config::from_env();
    tracing::info!(
        db_path = %config.db_path,
        max_fallback_lookups = config.max_fallback_lookups,
        inactivity_timeout_secs = config.inactivity_timeout_secs,
        "loaded config"
    );

    let conn = db::init_db(&config.db_path).expect("failed to initialize database");
    let state = Arc::new(AppState {
        db: Mutex::new(conn),
        config,
        lookup: Arc::new(wikidata::WikidataLookup),
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
