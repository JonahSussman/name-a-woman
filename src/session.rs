use axum::http::HeaderMap;
use sha2::{Digest, Sha256};

pub fn get_or_create_session_id(headers: &HeaderMap) -> (String, bool) {
    if let Some(cookie) = headers.get("cookie") {
        if let Ok(cookie_str) = cookie.to_str() {
            for part in cookie_str.split(';') {
                let part = part.trim();
                if let Some(value) = part.strip_prefix("naw_session=") {
                    return (value.to_string(), false);
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

pub fn session_cookie(session_id: &str) -> String {
    format!("naw_session={session_id}; HttpOnly; SameSite=Lax; Path=/; Max-Age=31536000")
}
