use std::env;

pub struct Config {
    pub max_fallback_lookups: i64,
    pub inactivity_timeout_secs: u64,
}

impl Config {
    pub fn from_env() -> Self {
        let max_fallback_lookups = env::var("NAW_MAX_FALLBACK_LOOKUPS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(i64::MAX);

        let inactivity_timeout_secs = env::var("NAW_INACTIVITY_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(300);

        Self {
            max_fallback_lookups,
            inactivity_timeout_secs,
        }
    }
}
