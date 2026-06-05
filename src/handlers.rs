use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;

use crate::AppState;
use crate::db;
use crate::models::*;

pub async fn health() -> StatusCode {
    StatusCode::OK
}

#[derive(Debug, serde::Deserialize)]
pub struct LeaderboardQuery {
    pub category: Option<Category>,
    pub count: Option<i64>,
    pub game_id: Option<String>,
    pub page: Option<i64>,
}

pub async fn leaderboard(
    State(state): State<Arc<AppState>>,
    Query(params): Query<LeaderboardQuery>,
) -> (StatusCode, Json<LeaderboardResponse>) {
    let category = params.category.unwrap_or(Category::Women);
    let cat = category.as_str();
    let count = params.count.unwrap_or(100);

    let db = state.db.lock().await;

    let top_10 = db::get_leaderboard_top(&db, cat, count, 10).unwrap_or_default();
    let bottom_10 = db::get_leaderboard_bottom(&db, cat, count, 10).unwrap_or_default();

    let your_neighborhood = params.game_id.as_deref().and_then(|gid| {
        db::get_neighborhood(&db, gid, cat, count, 10).ok().flatten()
    });

    let full_leaderboard = params.page.and_then(|page| {
        db::get_paginated_leaderboard(&db, cat, count, page, 50).ok()
    });

    (
        StatusCode::OK,
        Json(LeaderboardResponse {
            top_10,
            bottom_10,
            your_neighborhood,
            full_leaderboard,
        }),
    )
}

pub async fn game_stats(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    let db = state.db.lock().await;

    let game = match db::get_game(&db, &id) {
        Ok(Some(g)) => g,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "game not found"})),
            );
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            );
        }
    };

    let guesses = db::get_game_guesses(&db, &id).unwrap_or_default();

    let ranking = if game.completed_at.is_some() {
        db::get_rank(&db, &id, &game.category, game.target_count).ok()
    } else {
        None
    };

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "game": {
                "total_time_ms": game.total_time_ms,
                "category": game.category,
                "target_count": game.target_count,
                "accepted_count": game.accepted_count,
            },
            "guesses": guesses,
            "ranking": ranking,
        })),
    )
}
