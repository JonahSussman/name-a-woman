use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use axum::Router;
use axum::routing::get;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;

use name_a_woman::config::Config;
use name_a_woman::models::Person;
use name_a_woman::wikidata::PersonLookup;
use name_a_woman::{AppState, db, game, handlers};

struct MockLookup {
    people: HashMap<String, (Person, Vec<String>)>,
}

#[async_trait]
impl PersonLookup for MockLookup {
    async fn lookup_person(&self, name: &str) -> Option<(Person, Vec<String>)> {
        self.people.get(name).cloned()
    }
}

fn make_person(id: &str, name: &str, gender: &str) -> Person {
    Person {
        wikidata_id: id.to_string(),
        display_name: name.to_string(),
        gender: gender.to_string(),
        wikipedia_url: None,
        wikidata_url: format!("http://www.wikidata.org/entity/{id}"),
    }
}

const WOMEN: &[(&str, &str)] = &[
    ("Q7186", "Marie Curie"),
    ("Q5879", "Ada Lovelace"),
    ("Q11647", "Rosa Parks"),
    ("Q36322", "Frida Kahlo"),
    ("Q180099", "Amelia Earhart"),
    ("Q233591", "Malala Yousafzai"),
    ("Q1030", "Jane Austen"),
    ("Q295877", "Florence Nightingale"),
    ("Q170581", "Cleopatra"),
    ("Q47153", "Simone de Beauvoir"),
    ("Q37079", "Anne Frank"),
];

fn seed_db(conn: &rusqlite::Connection) {
    for &(id, name) in WOMEN {
        db::insert_person(conn, &make_person(id, name, "female")).unwrap();
        db::insert_name_variant(conn, id, name).unwrap();
    }
    db::insert_person(conn, &make_person("Q937", "Albert Einstein", "male")).unwrap();
    db::insert_name_variant(conn, "Q937", "Albert Einstein").unwrap();
}

async fn start_server(mock: MockLookup) -> u16 {
    let conn = db::init_db(":memory:").unwrap();
    seed_db(&conn);

    let config = Config {
        db_path: ":memory:".to_string(),
        max_fallback_lookups: 5,
        inactivity_timeout_secs: 300,
    };
    let state = Arc::new(AppState {
        db: Mutex::new(conn),
        config,
        lookup: Arc::new(mock),
    });

    let api = Router::new()
        .route("/game/ws", get(game::ws_handler))
        .route("/leaderboard", get(handlers::leaderboard))
        .route("/stats/game/{id}", get(handlers::game_stats))
        .route("/health", get(handlers::health));

    let app = Router::new().nest("/api", api).with_state(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    port
}

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect(port: u16) -> WsStream {
    let (ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/api/game/ws"))
        .await
        .unwrap();
    ws
}

async fn send_json(ws: &mut WsStream, value: &serde_json::Value) {
    let text = serde_json::to_string(value).unwrap();
    ws.send(Message::Text(text.into())).await.unwrap();
}

async fn recv_json(ws: &mut WsStream) -> serde_json::Value {
    let msg = ws.next().await.unwrap().unwrap();
    match msg {
        Message::Text(text) => serde_json::from_str(&text).unwrap(),
        other => panic!("expected text message, got {other:?}"),
    }
}

async fn start_game(ws: &mut WsStream, category: &str, target_count: i64) -> serde_json::Value {
    send_json(
        ws,
        &serde_json::json!({"action": "start", "category": category, "target_count": target_count}),
    )
    .await;
    let resp = recv_json(ws).await;
    assert_eq!(resp["type"], "started");
    resp
}

async fn guess(ws: &mut WsStream, name: &str) -> serde_json::Value {
    send_json(ws, &serde_json::json!({"action": "guess", "name": name})).await;
    recv_json(ws).await
}

#[tokio::test]
async fn test_accepted_guess() {
    let port = start_server(MockLookup {
        people: HashMap::new(),
    })
    .await;
    let mut ws = connect(port).await;
    start_game(&mut ws, "women", 10).await;

    let resp = guess(&mut ws, "Marie Curie").await;
    assert_eq!(resp["type"], "accepted");
    assert_eq!(resp["person"]["display_name"], "Marie Curie");
    assert_eq!(resp["accepted_count"], 1);
    assert_eq!(resp["game_complete"], false);
}

#[tokio::test]
async fn test_already_guessed() {
    let port = start_server(MockLookup {
        people: HashMap::new(),
    })
    .await;
    let mut ws = connect(port).await;
    start_game(&mut ws, "women", 10).await;

    guess(&mut ws, "Marie Curie").await;
    let resp = guess(&mut ws, "Marie Curie").await;
    assert_eq!(resp["type"], "already_guessed");
}

#[tokio::test]
async fn test_wrong_category() {
    let port = start_server(MockLookup {
        people: HashMap::new(),
    })
    .await;
    let mut ws = connect(port).await;
    start_game(&mut ws, "women", 10).await;

    let resp = guess(&mut ws, "Albert Einstein").await;
    assert_eq!(resp["type"], "wrong_category");
}

#[tokio::test]
async fn test_not_found() {
    let port = start_server(MockLookup {
        people: HashMap::new(),
    })
    .await;
    let mut ws = connect(port).await;
    start_game(&mut ws, "women", 10).await;

    let resp = guess(&mut ws, "xyznotaperson").await;
    assert_eq!(resp["type"], "not_found");
}

#[tokio::test]
async fn test_fallback_lookup() {
    let mut people = HashMap::new();
    people.insert(
        "Hypatia".to_string(),
        (make_person("Q44269", "Hypatia", "female"), vec![]),
    );
    let port = start_server(MockLookup { people }).await;
    let mut ws = connect(port).await;
    start_game(&mut ws, "women", 10).await;

    let resp = guess(&mut ws, "Hypatia").await;
    assert_eq!(resp["type"], "accepted");
    assert_eq!(resp["person"]["display_name"], "Hypatia");
    assert_eq!(resp["used_fallback"], true);
}

#[tokio::test]
async fn test_name_too_long() {
    let port = start_server(MockLookup {
        people: HashMap::new(),
    })
    .await;
    let mut ws = connect(port).await;
    start_game(&mut ws, "women", 10).await;

    let long_name = "a".repeat(201);
    let resp = guess(&mut ws, &long_name).await;
    assert_eq!(resp["type"], "error");
    assert_eq!(resp["message"], "name too long");
}

#[tokio::test]
async fn test_game_completion() {
    let port = start_server(MockLookup {
        people: HashMap::new(),
    })
    .await;
    let mut ws = connect(port).await;
    start_game(&mut ws, "women", 10).await;

    for &(_, name) in &WOMEN[..9] {
        let resp = guess(&mut ws, name).await;
        assert_eq!(resp["type"], "accepted");
        assert_eq!(resp["game_complete"], false);
    }

    let resp = guess(&mut ws, WOMEN[9].1).await;
    assert_eq!(resp["type"], "accepted");
    assert_eq!(resp["game_complete"], true);
    assert!(resp["completion"].is_object());
    assert_eq!(resp["completion"]["rank"], 1);
}
