CREATE TABLE IF NOT EXISTS people (
    wikidata_id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    gender TEXT NOT NULL,
    wikipedia_url TEXT,
    wikidata_url TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS name_variants (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    wikidata_id TEXT NOT NULL REFERENCES people(wikidata_id),
    name_normalized TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_name_variants_name ON name_variants(name_normalized);
CREATE INDEX IF NOT EXISTS idx_name_variants_wikidata ON name_variants(wikidata_id);

CREATE VIRTUAL TABLE IF NOT EXISTS name_fts USING fts5(
    wikidata_id UNINDEXED,
    name_normalized,
    tokenize='trigram'
);

CREATE TABLE IF NOT EXISTS games (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    ip_hash TEXT NOT NULL,
    category TEXT NOT NULL CHECK(category IN ('women', 'men', 'people')),
    target_count INTEGER NOT NULL CHECK(target_count IN (10, 100, 1000)),
    accepted_count INTEGER NOT NULL DEFAULT 0,
    fallback_lookups_used INTEGER NOT NULL DEFAULT 0,
    started_at TEXT NOT NULL,
    completed_at TEXT,
    total_time_ms INTEGER
);

CREATE TABLE IF NOT EXISTS guesses (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    game_id TEXT NOT NULL REFERENCES games(id),
    name_entered TEXT NOT NULL,
    person_id TEXT REFERENCES people(wikidata_id),
    accepted INTEGER NOT NULL DEFAULT 0,
    guess_time_ms INTEGER NOT NULL,
    guess_order INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_guesses_game ON guesses(game_id);
CREATE INDEX IF NOT EXISTS idx_games_category ON games(category, target_count, completed_at);
