use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;

use crate::AppState;
use crate::models::*;

pub async fn health() -> StatusCode {
    StatusCode::OK
}

pub async fn start_game(
    State(_state): State<Arc<AppState>>,
    Json(_req): Json<StartGameRequest>,
) -> (StatusCode, Json<StartGameResponse>) {
    // TODO
    (
        StatusCode::OK,
        Json(StartGameResponse {
            game_id: uuid::Uuid::new_v4().to_string(),
        }),
    )
}

pub async fn guess(
    State(_state): State<Arc<AppState>>,
    Path(_id): Path<String>,
    Json(_req): Json<GuessRequest>,
) -> (StatusCode, Json<GuessResponse>) {
    // TODO
    (
        StatusCode::OK,
        Json(GuessResponse {
            status: "not_found".to_string(),
            person: None,
            corrected_from: None,
            accepted_count: None,
            game_complete: None,
            completion: None,
            fallback_exhausted: None,
        }),
    )
}

#[derive(Debug, serde::Deserialize)]
pub struct LeaderboardQuery {
    pub category: Option<String>,
    pub count: Option<i64>,
    pub game_id: Option<String>,
    pub page: Option<i64>,
}

pub async fn leaderboard(
    State(_state): State<Arc<AppState>>,
    Query(_params): Query<LeaderboardQuery>,
) -> (StatusCode, Json<LeaderboardResponse>) {
    // TODO
    (
        StatusCode::OK,
        Json(LeaderboardResponse {
            top_10: vec![],
            bottom_10: vec![],
            your_neighborhood: None,
            full_leaderboard: None,
        }),
    )
}

pub async fn game_stats(
    State(_state): State<Arc<AppState>>,
    Path(_id): Path<String>,
) -> (StatusCode, Json<GameStatsResponse>) {
    // TODO
    (
        StatusCode::OK,
        Json(GameStatsResponse {
            game: GameSummary {
                total_time_ms: None,
                category: String::new(),
                target_count: 0,
                accepted_count: 0,
            },
            guesses: vec![],
            ranking: CompletionData {
                total_time_ms: 0,
                rank: 0,
                total_players: 0,
                percentile: 0.0,
            },
        }),
    )
}
