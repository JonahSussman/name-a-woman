use std::collections::HashSet;
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

#[derive(Debug, Deserialize)]
#[serde(tag = "action")]
enum ClientMessage {
    #[serde(rename = "start")]
    Start {
        category: Category,
        target_count: i64,
    },

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

struct GameState {
    guessed_ids: HashSet<String>,
    guesses: Vec<GuessRecord>,
    accepted_count: i64,
    fallback_lookups: i64,
}

struct GuessRecord {
    name_entered: String,
    person_id: String,
    guess_time_ms: i64,
    guess_order: i64,
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

async fn handle_game(
    mut socket: WebSocket,
    state: Arc<AppState>,
    user_id: String,
    ip_hash: String,
) {
    // Helper functions
    async fn send_error(socket: &mut WebSocket, message: impl Into<String>) {
        let _ = send(
            socket,
            &ServerMessage::Error {
                message: message.into(),
            },
        )
        .await;
    }

    async fn send_game_timeout(socket: &mut WebSocket, start: Instant, paused: Duration) {
        let elapsed = start.elapsed().saturating_sub(paused);
        let _ = send(
            socket,
            &ServerMessage::GameTimeout {
                elapsed_ms: elapsed.as_millis() as u64,
            },
        )
        .await;
    }

    // Get the first message
    let first_msg = match timeout(Duration::from_secs(30), socket.recv()).await {
        Ok(Some(Ok(Message::Text(text)))) => text,
        _ => return,
    };

    // Try to convert the json to a ClientMessage
    let start_msg: ClientMessage = match serde_json::from_str(&first_msg) {
        Ok(msg) => msg,
        Err(e) => {
            send_error(&mut socket, e.to_string()).await;
            return;
        }
    };

    // Try to convert the ClientMessage to a Start message and validate the fields
    let (category, target_count) = match start_msg {
        ClientMessage::Start {
            category,
            target_count,
        } => {
            if !matches!(target_count, 10 | 100 | 1000) {
                send_error(&mut socket, "invalid target_count").await;
                return;
            }
            (category, target_count)
        }
        _ => {
            send_error(&mut socket, "expected start action").await;
            return;
        }
    };

    let game_id = uuid::Uuid::new_v4().to_string();
    {
        let db = state.db.lock().await;
        if db::has_active_game(&db, &user_id).unwrap_or(false) {
            send_error(&mut socket, "you already have an active game").await;
            return;
        }

        if let Err(e) = db::create_game(
            &db,
            &game_id,
            &user_id,
            &ip_hash,
            category.as_str(),
            target_count,
        ) {
            send_error(&mut socket, e.to_string()).await;
            return;
        }
    }

    let _ = send(
        &mut socket,
        &ServerMessage::Started {
            game_id: game_id.clone(),
            max_fallbacks: state.config.max_fallback_lookups,
        },
    )
    .await;

    let time_start = Instant::now(); // When the game began
    let mut time_last_correct = Instant::now(); // When the last correct guess was accepted
    let mut time_total_paused = Duration::ZERO; // Cumulative time spent processing
    let mut time_paused_since_correct = Duration::ZERO; // Time spent processing since last correct guess
    let mut guess_order: i64 = 0; // Incremented on each guess
    let mut game = GameState {
        guessed_ids: HashSet::new(),
        guesses: Vec::new(),
        accepted_count: 0,
        fallback_lookups: 0,
    };
    let mut completed = false;

    loop {
        let time_remaining = Duration::from_secs(state.config.inactivity_timeout_secs)
            .saturating_sub(
                time_last_correct
                    .elapsed()
                    .saturating_sub(time_paused_since_correct),
            );

        if time_remaining.is_zero() {
            send_game_timeout(&mut socket, time_start, time_total_paused).await;
            break;
        }

        let msg = match timeout(time_remaining, socket.recv()).await {
            Ok(Some(Ok(Message::Text(text)))) => text,
            Ok(Some(Ok(Message::Close(_)))) | Ok(None) | Ok(Some(Err(_))) => break,
            Ok(Some(Ok(_))) => continue,
            Err(_) => {
                send_game_timeout(&mut socket, time_start, time_total_paused).await;
                break;
            }
        };

        let process_start = Instant::now();

        let guess_msg: ClientMessage = match serde_json::from_str(&msg) {
            Ok(msg) => msg,
            Err(e) => {
                let d = process_start.elapsed();
                time_total_paused += d;
                time_paused_since_correct += d;

                send_error(&mut socket, e.to_string()).await;
                continue;
            }
        };

        let name = match guess_msg {
            ClientMessage::Guess { name } => name,
            _ => {
                let d = process_start.elapsed();
                time_total_paused += d;
                time_paused_since_correct += d;

                send_error(&mut socket, "expected guess action").await;
                continue;
            }
        };

        if name.len() > 200 {
            let d = process_start.elapsed();
            time_total_paused += d;
            time_paused_since_correct += d;

            send_error(&mut socket, "name too long").await;
            continue;
        }

        let time_at_guess = time_start.elapsed().saturating_sub(time_total_paused);
        guess_order += 1;

        let mut response = process_guess(
            &state,
            &mut game,
            category,
            target_count,
            &name,
            time_at_guess,
            guess_order,
        )
        .await;

        let pause_duration = process_start.elapsed();
        time_total_paused += pause_duration;
        time_paused_since_correct += pause_duration;

        let is_accepted = matches!(&response, ServerMessage::Accepted { .. });
        let is_complete = matches!(
            &response,
            ServerMessage::Accepted {
                game_complete: true,
                ..
            }
        );

        if is_accepted {
            time_last_correct = Instant::now();
            time_paused_since_correct = Duration::ZERO;
        }

        if is_complete {
            completed = true;
            let total_time = time_start.elapsed().saturating_sub(time_total_paused);
            let completion =
                flush_game(&state, &game_id, &game, category, target_count, total_time).await;
            if let ServerMessage::Accepted {
                completion: ref mut c,
                ..
            } = response
            {
                *c = completion;
            }
            let _ = send(&mut socket, &response).await;
            break;
        }

        let _ = send(&mut socket, &response).await;
    }

    if !completed {
        let db = state.db.lock().await;
        let _ = db::abandon_game(&db, &game_id);
    }
}

async fn flush_game(
    state: &AppState,
    game_id: &str,
    game: &GameState,
    category: Category,
    target_count: i64,
    total_time: Duration,
) -> Option<CompletionData> {
    let db = state.db.lock().await;
    let guesses: Vec<(&str, &str, i64, i64)> = game
        .guesses
        .iter()
        .map(|g| {
            (
                g.name_entered.as_str(),
                g.person_id.as_str(),
                g.guess_time_ms,
                g.guess_order,
            )
        })
        .collect();
    let _ = db::flush_game_state(
        &db,
        game_id,
        &guesses,
        game.accepted_count,
        game.fallback_lookups,
    );

    let total_time_ms = total_time.as_millis() as i64;
    let _ = db::complete_game_with_time(&db, game_id, total_time_ms);
    db::get_rank(&db, game_id, category.as_str(), target_count).ok()
}

async fn process_guess(
    state: &AppState,
    game: &mut GameState,
    category: Category,
    target_count: i64,
    name: &str,
    time_at_guess: Duration,
    guess_order: i64,
) -> ServerMessage {
    let gender_filter = category.gender_filter();

    // Try exact match
    let db = state.db.lock().await;

    let matches = match db::lookup_exact(&db, name) {
        Ok(m) => m,
        Err(e) => {
            return ServerMessage::Error {
                message: e.to_string(),
            };
        }
    };

    match find_person(&matches, gender_filter, &game.guessed_ids) {
        Some(PersonMatch::Valid(person)) => {
            return accept_guess(
                game,
                target_count,
                name,
                &person,
                None,
                time_at_guess,
                guess_order,
                false,
            );
        }
        Some(PersonMatch::AlreadyGuessed(person)) => {
            return ServerMessage::AlreadyGuessed { person };
        }
        None => {}
    }

    if let Some(wrong) = matches.first() {
        if gender_filter.is_some() && wrong.gender != gender_filter.unwrap() {
            return ServerMessage::WrongCategory {
                person: wrong.clone(),
            };
        }
    }

    // Try fuzzy match
    let fuzzy_matches = match db::lookup_fuzzy(&db, name, 2) {
        Ok(m) => m,
        Err(e) => {
            return ServerMessage::Error {
                message: e.to_string(),
            };
        }
    };

    let fuzzy_people: Vec<Person> = fuzzy_matches.into_iter().map(|(p, _)| p).collect();
    if let Some(PersonMatch::Valid(person)) =
        find_person(&fuzzy_people, gender_filter, &game.guessed_ids)
    {
        let corrected_from = if normalize_name(name) != normalize_name(&person.display_name) {
            Some(name.to_string())
        } else {
            None
        };
        return accept_guess(
            game,
            target_count,
            name,
            &person,
            corrected_from,
            time_at_guess,
            guess_order,
            false,
        );
    }

    // Try Wikidata fallback
    if game.fallback_lookups >= state.config.max_fallback_lookups {
        return ServerMessage::NotFound {
            fallback_exhausted: Some(true),
        };
    }

    drop(db);

    let Some((person, aliases)) = state.lookup.lookup_person(name).await else {
        game.fallback_lookups += 1;
        return ServerMessage::NotFound {
            fallback_exhausted: None,
        };
    };

    let db = state.db.lock().await;
    let _ = db::insert_person(&db, &person);
    let _ = db::insert_name_variant(&db, &person.wikidata_id, &person.display_name);
    for alias in &aliases {
        let _ = db::insert_name_variant(&db, &person.wikidata_id, alias);
    }
    drop(db);

    game.fallback_lookups += 1;

    if game.guessed_ids.contains(&person.wikidata_id) {
        return ServerMessage::AlreadyGuessed { person };
    }

    if let Some(g) = category.gender_filter() {
        if person.gender != g {
            return ServerMessage::WrongCategory { person };
        }
    }

    accept_guess(
        game,
        target_count,
        name,
        &person,
        None,
        time_at_guess,
        guess_order,
        true,
    )
}

enum PersonMatch {
    Valid(Person),
    AlreadyGuessed(Person),
}

fn find_person(
    candidates: &[Person],
    gender_filter: Option<&str>,
    guessed_ids: &HashSet<String>,
) -> Option<PersonMatch> {
    let mut already_guessed = None;
    for person in candidates {
        if let Some(gender) = gender_filter {
            if person.gender != gender {
                continue;
            }
        }
        if guessed_ids.contains(&person.wikidata_id) {
            already_guessed.get_or_insert(person.clone());
        } else {
            return Some(PersonMatch::Valid(person.clone()));
        }
    }
    already_guessed.map(PersonMatch::AlreadyGuessed)
}

fn accept_guess(
    game: &mut GameState,
    target_count: i64,
    name_entered: &str,
    person: &Person,
    corrected_from: Option<String>,
    time_at_guess: Duration,
    guess_order: i64,
    used_fallback: bool,
) -> ServerMessage {
    game.guessed_ids.insert(person.wikidata_id.clone());
    game.accepted_count += 1;
    game.guesses.push(GuessRecord {
        name_entered: name_entered.to_string(),
        person_id: person.wikidata_id.clone(),
        guess_time_ms: time_at_guess.as_millis() as i64,
        guess_order,
    });

    let game_complete = game.accepted_count >= target_count;

    ServerMessage::Accepted {
        person: person.clone(),
        corrected_from,
        accepted_count: game.accepted_count,
        game_complete,
        completion: None,
        used_fallback,
    }
}

async fn send(socket: &mut WebSocket, msg: &ServerMessage) -> Result<(), axum::Error> {
    let text = serde_json::to_string(msg).unwrap();
    socket.send(Message::Text(text.into())).await
}
