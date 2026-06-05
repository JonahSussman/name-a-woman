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
use crate::wikidata;


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
    Started { game_id: String, max_fallbacks: i64 },
    #[serde(rename = "accepted")]
    Accepted {
        person: Person,
        #[serde(skip_serializing_if = "Option::is_none")]
        corrected_from: Option<String>,
        accepted_count: i64,
        game_complete: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        completion: Option<CompletionData>,
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        used_fallback: bool,
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
        if db::has_active_game(&db, &user_id).unwrap_or(false) {
            let _ = send(&mut socket, &ServerMessage::Error { message: "you already have an active game".into() }).await;
            return;
        }
        if let Err(e) = db::create_game(&db, &game_id, &user_id, &ip_hash, &category, target_count) {
            let _ = send(&mut socket, &ServerMessage::Error { message: e.to_string() }).await;
            return;
        }
    }

    let _ = send(&mut socket, &ServerMessage::Started { game_id: game_id.clone(), max_fallbacks: state.config.max_fallback_lookups }).await;

    let game_start = Instant::now();
    let mut last_correct = Instant::now();
    let mut guess_order: i64 = 0;
    let mut total_paused = Duration::ZERO;
    let mut paused_since_correct = Duration::ZERO;

    loop {
        let remaining = Duration::from_secs(state.config.inactivity_timeout_secs).saturating_sub(last_correct.elapsed().saturating_sub(paused_since_correct));
        if remaining.is_zero() {
            let player_elapsed = game_start.elapsed().saturating_sub(total_paused);
            let _ = send(&mut socket, &ServerMessage::GameTimeout {
                elapsed_ms: player_elapsed.as_millis() as u64,
            }).await;
            break;
        }

        let msg = match timeout(remaining, socket.recv()).await {
            Ok(Some(Ok(Message::Text(text)))) => text,
            Ok(Some(Ok(Message::Close(_)))) | Ok(None) => break,
            Ok(Some(Err(_))) => break,
            Ok(Some(Ok(_))) => continue,
            Err(_) => {
                let player_elapsed = game_start.elapsed().saturating_sub(total_paused);
                let _ = send(&mut socket, &ServerMessage::GameTimeout {
                    elapsed_ms: player_elapsed.as_millis() as u64,
                }).await;
                break;
            }
        };

        let process_start = Instant::now();

        let guess_msg: ClientMessage = match serde_json::from_str(&msg) {
            Ok(msg) => msg,
            Err(e) => {
                let d = process_start.elapsed();
                total_paused += d;
                paused_since_correct += d;
                let _ = send(&mut socket, &ServerMessage::Error { message: e.to_string() }).await;
                continue;
            }
        };

        let name = match guess_msg {
            ClientMessage::Guess { name } => name,
            _ => {
                let d = process_start.elapsed();
                total_paused += d;
                paused_since_correct += d;
                let _ = send(&mut socket, &ServerMessage::Error { message: "expected guess action".into() }).await;
                continue;
            }
        };

        let elapsed_ms = game_start.elapsed().saturating_sub(total_paused).as_millis() as i64;
        guess_order += 1;

        let (response, needs_fallback) = {
            let db = state.db.lock().await;
            let r = process_guess(&db, &game_id, &category, target_count, &name, elapsed_ms, guess_order);
            match r {
                GuessResult::Response(msg) => (msg, false),
                GuessResult::TryFallback => (ServerMessage::NotFound { fallback_exhausted: None }, true),
            }
        };

        let response = if needs_fallback {
            let fallback_count = {
                let db = state.db.lock().await;
                db::get_game(&db, &game_id).ok().flatten().map(|g| g.fallback_lookups_used).unwrap_or(5)
            };
            if fallback_count >= state.config.max_fallback_lookups {
                ServerMessage::NotFound { fallback_exhausted: Some(true) }
            } else if let Some((person, aliases)) = wikidata::lookup_person(&name).await {
                let db = state.db.lock().await;
                let _ = db::increment_fallback_lookups(&db, &game_id);
                let _ = db::insert_person(&db, &person);
                let _ = db::insert_name_variant(&db, &person.wikidata_id, &person.display_name);
                for alias in &aliases {
                    let _ = db::insert_name_variant(&db, &person.wikidata_id, alias);
                }

                if db::is_person_already_guessed(&db, &game_id, &person.wikidata_id).unwrap_or(false) {
                    ServerMessage::AlreadyGuessed { person }
                } else {
                    let gender_filter = match category.as_str() {
                        "women" => Some("female"),
                        "men" => Some("male"),
                        _ => None,
                    };
                    if let Some(g) = gender_filter {
                        if person.gender != g {
                            ServerMessage::WrongCategory { person }
                        } else {
                            accept_guess(&db, &game_id, &category, target_count, &name, &person, None, elapsed_ms, guess_order, true)
                        }
                    } else {
                        accept_guess(&db, &game_id, &category, target_count, &name, &person, None, elapsed_ms, guess_order, true)
                    }
                }
            } else {
                let db = state.db.lock().await;
                let _ = db::increment_fallback_lookups(&db, &game_id);
                ServerMessage::NotFound { fallback_exhausted: None }
            }
        } else {
            response
        };

        let pause_duration = process_start.elapsed();
        total_paused += pause_duration;
        paused_since_correct += pause_duration;

        let is_accepted = matches!(&response, ServerMessage::Accepted { .. });
        let is_complete = matches!(&response, ServerMessage::Accepted { game_complete: true, .. });

        if is_accepted {
            last_correct = Instant::now();
            paused_since_correct = Duration::ZERO;
        }

        let _ = send(&mut socket, &response).await;

        if is_complete {
            break;
        }
    }

    // Mark incomplete games as abandoned so they don't block new games
    let db = state.db.lock().await;
    let _ = db::abandon_game(&db, &game_id);
}

enum GuessResult {
    Response(ServerMessage),
    TryFallback,
}

fn process_guess(
    db: &rusqlite::Connection,
    game_id: &str,
    category: &str,
    target_count: i64,
    name: &str,
    elapsed_ms: i64,
    guess_order: i64,
) -> GuessResult {
    let gender_filter = match category {
        "women" => Some("female"),
        "men" => Some("male"),
        "people" => None,
        _ => return GuessResult::Response(ServerMessage::Error { message: "invalid category".into() }),
    };

    let matches = match db::lookup_exact(db, name) {
        Ok(m) => m,
        Err(e) => return GuessResult::Response(ServerMessage::Error { message: e.to_string() }),
    };

    if let Some(person) = find_valid_person(&matches, gender_filter, db, game_id) {
        return GuessResult::Response(accept_guess(db, game_id, category, target_count, name, &person, None, elapsed_ms, guess_order, false));
    }

    if let Some(person) = find_already_guessed(&matches, gender_filter, db, game_id) {
        return GuessResult::Response(ServerMessage::AlreadyGuessed { person });
    }

    if let Some(wrong) = matches.first() {
        if gender_filter.is_some() && wrong.gender != gender_filter.unwrap() {
            return GuessResult::Response(ServerMessage::WrongCategory { person: wrong.clone() });
        }
    }

    let fuzzy_matches = match db::lookup_fuzzy(db, name, 2) {
        Ok(m) => m,
        Err(e) => return GuessResult::Response(ServerMessage::Error { message: e.to_string() }),
    };

    let fuzzy_people: Vec<Person> = fuzzy_matches.into_iter().map(|(p, _)| p).collect();
    if let Some(person) = find_valid_person(&fuzzy_people, gender_filter, db, game_id) {
        let corrected_from = if normalize_name(name) != normalize_name(&person.display_name) {
            Some(name.to_string())
        } else {
            None
        };
        return GuessResult::Response(accept_guess(db, game_id, category, target_count, name, &person, corrected_from, elapsed_ms, guess_order, false));
    }

    GuessResult::TryFallback
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

fn find_already_guessed(
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
            Ok(true) => return Some(person.clone()),
            _ => continue,
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
    used_fallback: bool,
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
        used_fallback,
    }
}

async fn send(socket: &mut WebSocket, msg: &ServerMessage) -> Result<(), axum::Error> {
    let text = serde_json::to_string(msg).unwrap();
    socket.send(Message::Text(text.into())).await
}
