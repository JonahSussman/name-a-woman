pub mod config;
pub mod db;
pub mod game;
pub mod handlers;
pub mod models;
pub mod normalize;
pub mod session;
pub mod wikidata;

use std::sync::Arc;
use tokio::sync::Mutex;

pub struct AppState {
    pub db: Mutex<rusqlite::Connection>,
    pub config: config::Config,
    pub lookup: Arc<dyn wikidata::PersonLookup>,
}
