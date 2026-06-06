use axum::http::HeaderMap;
use rustrict::CensorStr;
use sha2::{Digest, Sha256};

const MAX_USER_ID_LEN: usize = 36;

pub fn is_valid_user_id(id: &str) -> bool {
    id.len() >= 3
        && id.len() <= MAX_USER_ID_LEN
        && id.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && !id.is_inappropriate()
}

pub fn get_or_create_session_id(headers: &HeaderMap) -> (String, bool) {
    if let Some(cookie) = headers.get("cookie") {
        if let Ok(cookie_str) = cookie.to_str() {
            for part in cookie_str.split(';') {
                let part = part.trim();
                if let Some(value) = part.strip_prefix("naw_session=") {
                    if is_valid_user_id(value) {
                        return (value.to_string(), false);
                    }
                }
            }
        }
    }
    (uuid::Uuid::new_v4().to_string(), true)
}

pub fn hash_ip(headers: &HeaderMap) -> String {
    let ip = headers
        .get("x-real-ip")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown");

    let mut hasher = Sha256::new();
    hasher.update(ip.as_bytes());
    let result = hasher.finalize();
    hex::encode(&result[..8])
}
