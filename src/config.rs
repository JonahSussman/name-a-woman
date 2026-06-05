use std::env;

pub struct Config {
    pub db_path: String,
    pub max_fallback_lookups: i64,
    pub inactivity_timeout_secs: u64,
}

impl Config {
    pub fn from_env() -> Self {
        let db_path = env::var("NAW_DB_PATH").unwrap_or_else(|_| "data/names.db".to_string());

        let max_fallback_lookups = env::var("NAW_MAX_FALLBACK_LOOKUPS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(i64::MAX);

        let inactivity_timeout_secs = env::var("NAW_INACTIVITY_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(300);

        Self {
            db_path,
            max_fallback_lookups,
            inactivity_timeout_secs,
        }
    }
}
