use std::sync::Arc;
use std::time::Instant;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use tokio::time::{Duration, timeout};

use crate::AppState;
use crate::db;
use crate::models::*;
use crate::normalize::normalize_name;
use crate::session;

const INACTIVITY_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Deserialize)]
#[serde(tag = "action")]
enum ClientMessage {
    #[serde(rename = "start")]
    Start { category: String, target_count: i64 },
    #[serde(rename = "guess")]
    Guess { name: String },
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum ServerMessage {
    #[serde(rename = "started")]
    Started { game_id: String },
    #[serde(rename = "accepted")]
    Accepted {
        person: Person,
        #[serde(skip_serializing_if = "Option::is_none")]
        corrected_from: Option<String>,
        accepted_count: i64,
        game_complete: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        completion: Option<CompletionData>,
    },
    #[serde(rename = "already_guessed")]
    AlreadyGuessed { person: Person },
    #[serde(rename = "wrong_category")]
    WrongCategory { person: Person },
    #[serde(rename = "not_found")]
    NotFound {
        #[serde(skip_serializing_if = "Option::is_none")]
        fallback_exhausted: Option<bool>,
    },
    #[serde(rename = "game_timeout")]
    GameTimeout { elapsed_ms: u64 },
    #[serde(rename = "error")]
    Error { message: String },
}

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let (user_id, _is_new) = session::get_or_create_session_id(&headers);
    let ip_hash = session::hash_ip(&headers);
    ws.on_upgrade(move |socket| handle_game(socket, state, user_id, ip_hash))
}

async fn handle_game(mut socket: WebSocket, state: Arc<AppState>, user_id: String, ip_hash: String) {
    let first_msg = match timeout(Duration::from_secs(30), socket.recv()).await {
        Ok(Some(Ok(Message::Text(text)))) => text,
        _ => return,
    };

    let start_msg: ClientMessage = match serde_json::from_str(&first_msg) {
        Ok(msg) => msg,
        Err(e) => {
            let _ = send(&mut socket, &ServerMessage::Error { message: e.to_string() }).await;
            return;
        }
    };

    let (category, target_count) = match start_msg {
        ClientMessage::Start { category, target_count } => {
            if !matches!(category.as_str(), "women" | "men" | "people") {
                let _ = send(&mut socket, &ServerMessage::Error { message: "invalid category".into() }).await;
                return;
            }
            if !matches!(target_count, 10 | 100 | 1000) {
                let _ = send(&mut socket, &ServerMessage::Error { message: "invalid target_count".into() }).await;
                return;
            }
            (category, target_count)
        }
        _ => {
            let _ = send(&mut socket, &ServerMessage::Error { message: "expected start action".into() }).await;
            return;
        }
    };

    let game_id = uuid::Uuid::new_v4().to_string();
    {
        let db = state.db.lock().await;
        if let Err(e) = db::create_game(&db, &game_id, &user_id, &ip_hash, &category, target_count) {
            let _ = send(&mut socket, &ServerMessage::Error { message: e.to_string() }).await;
            return;
        }
    }

    let _ = send(&mut socket, &ServerMessage::Started { game_id: game_id.clone() }).await;

    let game_start = Instant::now();
    let mut last_correct = Instant::now();
    let mut guess_order: i64 = 0;

    loop {
        let remaining = INACTIVITY_TIMEOUT.saturating_sub(last_correct.elapsed());
        if remaining.is_zero() {
            let _ = send(&mut socket, &ServerMessage::GameTimeout {
                elapsed_ms: game_start.elapsed().as_millis() as u64,
            }).await;
            break;
        }

        let msg = match timeout(remaining, socket.recv()).await {
            Ok(Some(Ok(Message::Text(text)))) => text,
            Ok(Some(Ok(Message::Close(_)))) | Ok(None) => break,
            Ok(Some(Err(_))) => break,
            Ok(Some(Ok(_))) => continue,
            Err(_) => {
                let _ = send(&mut socket, &ServerMessage::GameTimeout {
                    elapsed_ms: game_start.elapsed().as_millis() as u64,
                }).await;
                break;
            }
        };

        let guess_msg: ClientMessage = match serde_json::from_str(&msg) {
            Ok(msg) => msg,
            Err(e) => {
                let _ = send(&mut socket, &ServerMessage::Error { message: e.to_string() }).await;
                continue;
            }
        };

        let name = match guess_msg {
            ClientMessage::Guess { name } => name,
            _ => {
                let _ = send(&mut socket, &ServerMessage::Error { message: "expected guess action".into() }).await;
                continue;
            }
        };

        let elapsed_ms = game_start.elapsed().as_millis() as i64;
        guess_order += 1;

        let response = {
            let db = state.db.lock().await;
            process_guess(&db, &game_id, &category, target_count, &name, elapsed_ms, guess_order)
        };

        let is_accepted = matches!(&response, ServerMessage::Accepted { .. });
        let is_complete = matches!(&response, ServerMessage::Accepted { game_complete: true, .. });

        if is_accepted {
            last_correct = Instant::now();
        }

        let _ = send(&mut socket, &response).await;

        if is_complete {
            break;
        }
    }
}

fn process_guess(
    db: &rusqlite::Connection,
    game_id: &str,
    category: &str,
    target_count: i64,
    name: &str,
    elapsed_ms: i64,
    guess_order: i64,
) -> ServerMessage {
    let gender_filter = match category {
        "women" => Some("female"),
        "men" => Some("male"),
        "people" => None,
        _ => return ServerMessage::Error { message: "invalid category".into() },
    };

    let matches = match db::lookup_exact(db, name) {
        Ok(m) => m,
        Err(e) => return ServerMessage::Error { message: e.to_string() },
    };

    if let Some(person) = find_valid_person(&matches, gender_filter, db, game_id) {
        return accept_guess(db, game_id, category, target_count, name, &person, None, elapsed_ms, guess_order);
    }

    if let Some(wrong) = matches.first() {
        if gender_filter.is_some() && wrong.gender != gender_filter.unwrap() {
            let _ = db::insert_guess(db, game_id, name, Some(&wrong.wikidata_id), false, elapsed_ms, guess_order);
            return ServerMessage::WrongCategory { person: wrong.clone() };
        }
    }

    let fuzzy_matches = match db::lookup_fuzzy(db, name, 2) {
        Ok(m) => m,
        Err(e) => return ServerMessage::Error { message: e.to_string() },
    };

    let fuzzy_people: Vec<Person> = fuzzy_matches.into_iter().map(|(p, _)| p).collect();
    if let Some(person) = find_valid_person(&fuzzy_people, gender_filter, db, game_id) {
        let corrected_from = if normalize_name(name) != normalize_name(&person.display_name) {
            Some(name.to_string())
        } else {
            None
        };
        return accept_guess(db, game_id, category, target_count, name, &person, corrected_from, elapsed_ms, guess_order);
    }

    // TODO: Wikidata fallback (Phase 3)

    let _ = db::insert_guess(db, game_id, name, None, false, elapsed_ms, guess_order);
    ServerMessage::NotFound { fallback_exhausted: None }
}

fn find_valid_person(
    candidates: &[Person],
    gender_filter: Option<&str>,
    db: &rusqlite::Connection,
    game_id: &str,
) -> Option<Person> {
    for person in candidates {
        if let Some(gender) = gender_filter {
            if person.gender != gender {
                continue;
            }
        }
        match db::is_person_already_guessed(db, game_id, &person.wikidata_id) {
            Ok(true) => continue,
            Ok(false) => return Some(person.clone()),
            Err(_) => continue,
        }
    }
    None
}

fn accept_guess(
    db: &rusqlite::Connection,
    game_id: &str,
    category: &str,
    target_count: i64,
    name_entered: &str,
    person: &Person,
    corrected_from: Option<String>,
    elapsed_ms: i64,
    guess_order: i64,
) -> ServerMessage {
    if let Err(e) = db::insert_guess(db, game_id, name_entered, Some(&person.wikidata_id), true, elapsed_ms, guess_order) {
        return ServerMessage::Error { message: e.to_string() };
    }

    let accepted_count = match db::increment_accepted_count(db, game_id) {
        Ok(c) => c,
        Err(e) => return ServerMessage::Error { message: e.to_string() },
    };

    let game_complete = accepted_count >= target_count;
    let completion = if game_complete {
        let _ = db::complete_game(db, game_id);
        db::get_rank(db, game_id, category, target_count).ok()
    } else {
        None
    };

    ServerMessage::Accepted {
        person: person.clone(),
        corrected_from,
        accepted_count,
        game_complete,
        completion,
    }
}

async fn send(socket: &mut WebSocket, msg: &ServerMessage) -> Result<(), axum::Error> {
    let text = serde_json::to_string(msg).unwrap();
    socket.send(Message::Text(text.into())).await
}
