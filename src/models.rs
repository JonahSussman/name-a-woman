use std::fmt;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "frontend/types.gen.ts")]
pub enum Category {
    #[serde(rename = "women")]
    Women,
    #[serde(rename = "men")]
    Men,
    #[serde(rename = "people")]
    People,
}

impl Category {
    pub fn gender_filter(self) -> Option<&'static str> {
        match self {
            Category::Women => Some("female"),
            Category::Men => Some("male"),
            Category::People => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Category::Women => "women",
            Category::Men => "men",
            Category::People => "people",
        }
    }
}

impl fmt::Display for Category {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "frontend/types.gen.ts")]
pub struct Person {
    pub wikidata_id: String,
    pub display_name: String,
    pub gender: String,
    pub wikipedia_url: Option<String>,
    pub wikidata_url: String,
}

#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "frontend/types.gen.ts")]
pub struct CompletionData {
    pub total_time_ms: i64,
    pub rank: i64,
    pub total_players: i64,
    pub percentile: f64,
}

#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "frontend/types.gen.ts")]
pub struct LeaderboardEntry {
    pub rank: i64,
    pub game_id: String,
    pub user_id: String,
    pub total_time_ms: i64,
    pub accepted_count: i64,
    pub category: String,
    pub is_you: bool,
}

#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "frontend/types.gen.ts")]
pub struct LeaderboardResponse {
    pub top_10: Vec<LeaderboardEntry>,
    pub bottom_10: Vec<LeaderboardEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub your_neighborhood: Option<Neighborhood>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full_leaderboard: Option<PaginatedLeaderboard>,
}

#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "frontend/types.gen.ts")]
pub struct Neighborhood {
    pub above: Vec<LeaderboardEntry>,
    pub you: LeaderboardEntry,
    pub below: Vec<LeaderboardEntry>,
}

#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "frontend/types.gen.ts")]
pub struct PaginatedLeaderboard {
    pub entries: Vec<LeaderboardEntry>,
    pub page: i64,
    pub total_pages: i64,
    pub total_entries: i64,
}

#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "frontend/types.gen.ts")]
pub struct GuessSummary {
    pub order: i64,
    pub display_name: String,
    pub guess_time_ms: i64,
    pub wikipedia_url: Option<String>,
    pub wikidata_url: String,
}

#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "frontend/types.gen.ts")]
pub struct GameSummary {
    pub total_time_ms: Option<i64>,
    pub category: String,
    pub target_count: i64,
    pub accepted_count: i64,
}

#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "frontend/types.gen.ts")]
pub struct GameStatsResponse {
    pub game: GameSummary,
    pub guesses: Vec<GuessSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ranking: Option<CompletionData>,
}
