use rusqlite::{Connection, Result, params};

use crate::models::*;
use crate::normalize::normalize_name;

pub fn init_db(path: &str) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL;")?;
    migrate(&conn)?;
    Ok(conn)
}

fn get_schema_version(conn: &Connection) -> i64 {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL DEFAULT 0)",
    )
    .ok();
    conn.query_row("SELECT version FROM schema_version", [], |row| row.get(0))
        .unwrap_or(0)
}

fn set_schema_version(conn: &Connection, version: i64) -> Result<()> {
    conn.execute_batch(
        "DELETE FROM schema_version; INSERT INTO schema_version (version) VALUES (0)",
    )?;
    conn.execute("UPDATE schema_version SET version = ?1", params![version])?;
    Ok(())
}

fn migrate(conn: &Connection) -> Result<()> {
    let version = get_schema_version(conn);

    if version < 1 {
        conn.execute_batch(include_str!("../schema.sql"))?;
        backfill_fts(conn)?;
        set_schema_version(conn, 1)?;
    }

    // Abandon any games left active from a previous run
    conn.execute_batch(
        "UPDATE games SET completed_at = datetime('now') WHERE completed_at IS NULL",
    )?;

    Ok(())
}

fn backfill_fts(conn: &Connection) -> Result<()> {
    let fts_count: i64 = conn.query_row("SELECT COUNT(*) FROM name_fts", [], |row| row.get(0))?;
    if fts_count > 0 {
        return Ok(());
    }
    let variant_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM name_variants", [], |row| row.get(0))?;
    if variant_count == 0 {
        return Ok(());
    }
    conn.execute_batch(
        "INSERT INTO name_fts (wikidata_id, name_normalized)
         SELECT wikidata_id, name_normalized FROM name_variants",
    )?;
    conn.execute_batch(
        "INSERT INTO name_fts (wikidata_id, name_normalized)
         SELECT wikidata_id, REPLACE(name_normalized, ' ', '')
         FROM name_variants
         WHERE name_normalized LIKE '% %'",
    )?;
    conn.execute_batch(
        "INSERT INTO name_variants (wikidata_id, name_normalized)
         SELECT wikidata_id, REPLACE(name_normalized, ' ', '')
         FROM name_variants
         WHERE name_normalized LIKE '% %'",
    )?;
    Ok(())
}

fn person_from_row(row: &rusqlite::Row) -> Result<Person> {
    Ok(Person {
        wikidata_id: row.get(0)?,
        display_name: row.get(1)?,
        gender: row.get(2)?,
        wikipedia_url: row.get(3)?,
        wikidata_url: row.get(4)?,
    })
}

pub fn insert_person(conn: &Connection, person: &Person) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO people (wikidata_id, display_name, gender, wikipedia_url, wikidata_url)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            person.wikidata_id,
            person.display_name,
            person.gender,
            person.wikipedia_url,
            person.wikidata_url,
        ],
    )?;
    Ok(())
}

pub fn insert_name_variant(conn: &Connection, wikidata_id: &str, name: &str) -> Result<()> {
    let normalized = normalize_name(name);
    insert_variant_if_new(conn, wikidata_id, &normalized)?;
    let compact = normalized.replace(' ', "");
    if compact != normalized {
        insert_variant_if_new(conn, wikidata_id, &compact)?;
    }
    Ok(())
}

fn insert_variant_if_new(conn: &Connection, wikidata_id: &str, normalized: &str) -> Result<()> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM name_variants WHERE wikidata_id = ?1 AND name_normalized = ?2)",
        params![wikidata_id, normalized],
        |row| row.get(0),
    )?;
    if !exists {
        conn.execute(
            "INSERT INTO name_variants (wikidata_id, name_normalized) VALUES (?1, ?2)",
            params![wikidata_id, normalized],
        )?;
        conn.execute(
            "INSERT INTO name_fts (wikidata_id, name_normalized) VALUES (?1, ?2)",
            params![wikidata_id, normalized],
        )?;
    }
    Ok(())
}

pub fn lookup_exact(conn: &Connection, name: &str) -> Result<Vec<Person>> {
    let normalized = normalize_name(name);
    let mut stmt = conn.prepare(
        "SELECT p.wikidata_id, p.display_name, p.gender, p.wikipedia_url, p.wikidata_url
         FROM name_variants nv
         JOIN people p ON nv.wikidata_id = p.wikidata_id
         WHERE nv.name_normalized = ?1",
    )?;
    let rows = stmt.query_map(params![normalized], person_from_row)?;
    rows.collect()
}

pub fn lookup_fuzzy(
    conn: &Connection,
    name: &str,
    max_distance: usize,
) -> Result<Vec<(Person, String)>> {
    let normalized = normalize_name(name);
    let compact = normalized.replace(' ', "");

    if normalized.len() < 3 {
        return Ok(Vec::new());
    }

    let mut candidates = Vec::new();
    let mut seen = std::collections::HashSet::new();

    fts_search(conn, &normalized, &mut candidates, &mut seen)?;
    if compact != normalized {
        fts_search(conn, &compact, &mut candidates, &mut seen)?;
    }
    for word in normalized.split_whitespace() {
        if word.len() >= 4 {
            fts_search(conn, word, &mut candidates, &mut seen)?;
        }
    }

    let window = 5.min(normalized.len());
    if candidates.is_empty() && normalized.len() > window {
        for start in 0..=normalized.len() - window {
            fts_search(
                conn,
                &normalized[start..start + window],
                &mut candidates,
                &mut seen,
            )?;
        }
    }
    if candidates.is_empty() && compact != normalized && compact.len() > window {
        for start in 0..=compact.len() - window {
            fts_search(
                conn,
                &compact[start..start + window],
                &mut candidates,
                &mut seen,
            )?;
        }
    }

    let mut results: Vec<(Person, String)> = candidates
        .into_iter()
        .filter(|(_, variant)| {
            let dist = strsim::levenshtein(&normalized, variant);
            let dist_compact = strsim::levenshtein(&compact, &variant.replace(' ', ""));
            dist.min(dist_compact) <= max_distance
        })
        .collect();

    results.sort_by_key(|(_, variant)| {
        let dist = strsim::levenshtein(&normalized, variant);
        let dist_compact = strsim::levenshtein(&compact, &variant.replace(' ', ""));
        dist.min(dist_compact)
    });

    Ok(results)
}

fn fts_search(
    conn: &Connection,
    query: &str,
    results: &mut Vec<(Person, String)>,
    seen: &mut std::collections::HashSet<(String, String)>,
) -> Result<()> {
    if query.len() < 3 {
        return Ok(());
    }

    let escaped = query.replace('"', "\"\"");
    let fts_query = format!("\"{escaped}\"");

    let mut stmt = conn.prepare(
        "SELECT p.wikidata_id, p.display_name, p.gender, p.wikipedia_url, p.wikidata_url, nf.name_normalized
         FROM name_fts nf
         JOIN people p ON nf.wikidata_id = p.wikidata_id
         WHERE name_fts MATCH ?1
         LIMIT 50",
    )?;

    let rows = stmt.query_map(params![fts_query], |row| {
        Ok((person_from_row(row)?, row.get::<_, String>(5)?))
    })?;

    for row in rows {
        let (person, variant) = row?;
        if seen.insert((person.wikidata_id.clone(), variant.clone())) {
            results.push((person, variant));
        }
    }

    Ok(())
}

pub fn has_active_game(conn: &Connection, user_id: &str) -> Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM games WHERE user_id = ?1 AND completed_at IS NULL
         AND started_at > datetime('now', '-10 minutes')",
        params![user_id],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

pub fn create_game(
    conn: &Connection,
    game_id: &str,
    user_id: &str,
    ip_hash: &str,
    category: &str,
    target_count: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO games (id, user_id, ip_hash, category, target_count, started_at)
         VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))",
        params![game_id, user_id, ip_hash, category, target_count],
    )?;
    Ok(())
}

pub struct GameRow {
    pub id: String,
    pub user_id: String,
    pub category: String,
    pub target_count: i64,
    pub accepted_count: i64,
    pub completed_at: Option<String>,
    pub total_time_ms: Option<i64>,
}

pub fn get_game(conn: &Connection, game_id: &str) -> Result<Option<GameRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, category, target_count, accepted_count, completed_at, total_time_ms
         FROM games WHERE id = ?1",
    )?;
    let mut rows = stmt.query_map(params![game_id], |row| {
        Ok(GameRow {
            id: row.get(0)?,
            user_id: row.get(1)?,
            category: row.get(2)?,
            target_count: row.get(3)?,
            accepted_count: row.get(4)?,
            completed_at: row.get(5)?,
            total_time_ms: row.get(6)?,
        })
    })?;
    match rows.next() {
        Some(row) => Ok(Some(row?)),
        None => Ok(None),
    }
}

pub fn flush_game_state(
    conn: &Connection,
    game_id: &str,
    guesses: &[(&str, &str, i64, i64)],
    accepted_count: i64,
    fallback_lookups: i64,
) -> Result<()> {
    conn.execute_batch("BEGIN")?;
    {
        let mut stmt = conn.prepare(
            "INSERT INTO guesses (game_id, name_entered, person_id, accepted, guess_time_ms, guess_order)
             VALUES (?1, ?2, ?3, 1, ?4, ?5)",
        )?;
        for (name_entered, person_id, guess_time_ms, guess_order) in guesses {
            stmt.execute(params![game_id, name_entered, person_id, guess_time_ms, guess_order])?;
        }
    }
    conn.execute(
        "UPDATE games SET accepted_count = ?2, fallback_lookups_used = ?3 WHERE id = ?1",
        params![game_id, accepted_count, fallback_lookups],
    )?;
    conn.execute_batch("COMMIT")?;
    Ok(())
}

pub fn abandon_game(conn: &Connection, game_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE games SET completed_at = datetime('now')
         WHERE id = ?1 AND completed_at IS NULL",
        params![game_id],
    )?;
    Ok(())
}


pub fn complete_game_with_time(
    conn: &Connection,
    game_id: &str,
    total_time_ms: i64,
) -> Result<()> {
    conn.execute(
        "UPDATE games SET completed_at = datetime('now'), total_time_ms = ?2 WHERE id = ?1",
        params![game_id, total_time_ms],
    )?;
    Ok(())
}

pub fn get_rank(
    conn: &Connection,
    game_id: &str,
    category: &str,
    target_count: i64,
) -> Result<CompletionData> {
    let total_time_ms: i64 = conn.query_row(
        "SELECT total_time_ms FROM games WHERE id = ?1",
        params![game_id],
        |row| row.get(0),
    )?;

    let rank: i64 = conn.query_row(
        "SELECT COUNT(*) + 1 FROM games
         WHERE category = ?1 AND target_count = ?2 AND completed_at IS NOT NULL AND total_time_ms IS NOT NULL
               AND total_time_ms < ?3 AND id != ?4",
        params![category, target_count, total_time_ms, game_id],
        |row| row.get(0),
    )?;

    let total_players: i64 = conn.query_row(
        "SELECT COUNT(*) FROM games
         WHERE category = ?1 AND target_count = ?2 AND completed_at IS NOT NULL AND total_time_ms IS NOT NULL",
        params![category, target_count],
        |row| row.get(0),
    )?;

    let percentile = if total_players > 0 {
        ((total_players - rank) as f64 / total_players as f64) * 100.0
    } else {
        100.0
    };

    Ok(CompletionData {
        total_time_ms,
        rank,
        total_players,
        percentile,
    })
}

fn leaderboard_row(row: &rusqlite::Row) -> Result<(String, String, i64, i64, String)> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
    ))
}

fn to_leaderboard_entry(
    rank: i64,
    t: (String, String, i64, i64, String),
    is_you: bool,
) -> LeaderboardEntry {
    LeaderboardEntry {
        rank,
        game_id: t.0,
        user_id: t.1,
        total_time_ms: t.2,
        accepted_count: t.3,
        category: t.4,
        is_you,
    }
}

pub fn get_leaderboard_top(
    conn: &Connection,
    category: &str,
    target_count: i64,
    limit: i64,
) -> Result<Vec<LeaderboardEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, total_time_ms, accepted_count, category
         FROM games
         WHERE category = ?1 AND target_count = ?2 AND completed_at IS NOT NULL AND total_time_ms IS NOT NULL
         ORDER BY total_time_ms ASC
         LIMIT ?3",
    )?;
    let rows: Vec<_> = stmt
        .query_map(params![category, target_count, limit], leaderboard_row)?
        .collect::<Result<Vec<_>>>()?;

    Ok(rows
        .into_iter()
        .enumerate()
        .map(|(i, t)| to_leaderboard_entry((i + 1) as i64, t, false))
        .collect())
}

pub fn get_leaderboard_bottom(
    conn: &Connection,
    category: &str,
    target_count: i64,
    limit: i64,
) -> Result<Vec<LeaderboardEntry>> {
    let total: i64 = conn.query_row(
        "SELECT COUNT(*) FROM games WHERE category = ?1 AND target_count = ?2 AND completed_at IS NOT NULL AND total_time_ms IS NOT NULL",
        params![category, target_count],
        |row| row.get(0),
    )?;

    let mut stmt = conn.prepare(
        "SELECT id, user_id, total_time_ms, accepted_count, category
         FROM games
         WHERE category = ?1 AND target_count = ?2 AND completed_at IS NOT NULL AND total_time_ms IS NOT NULL
         ORDER BY total_time_ms DESC
         LIMIT ?3",
    )?;
    let rows: Vec<_> = stmt
        .query_map(params![category, target_count, limit], leaderboard_row)?
        .collect::<Result<Vec<_>>>()?;

    let start_rank = total - rows.len() as i64 + 1;
    Ok(rows
        .into_iter()
        .rev()
        .enumerate()
        .map(|(i, t)| to_leaderboard_entry(start_rank + i as i64, t, false))
        .collect())
}

pub fn get_neighborhood(
    conn: &Connection,
    game_id: &str,
    category: &str,
    target_count: i64,
    range: i64,
) -> Result<Option<Neighborhood>> {
    let game = match get_game(conn, game_id)? {
        Some(g) if g.completed_at.is_some() => g,
        _ => return Ok(None),
    };
    let total_time_ms = game.total_time_ms.unwrap_or(0);

    let rank_data = get_rank(conn, game_id, category, target_count)?;
    let my_rank = rank_data.rank;

    let mut above_stmt = conn.prepare(
        "SELECT id, user_id, total_time_ms, accepted_count, category
         FROM games
         WHERE category = ?1 AND target_count = ?2 AND completed_at IS NOT NULL AND total_time_ms IS NOT NULL
               AND (total_time_ms < ?3 OR (total_time_ms = ?3 AND id < ?4))
         ORDER BY total_time_ms DESC
         LIMIT ?5",
    )?;
    let above_rows: Vec<_> = above_stmt
        .query_map(
            params![category, target_count, total_time_ms, game_id, range],
            leaderboard_row,
        )?
        .collect::<Result<Vec<_>>>()?;

    let above_start = my_rank - above_rows.len() as i64;
    let above: Vec<_> = above_rows
        .into_iter()
        .rev()
        .enumerate()
        .map(|(i, t)| to_leaderboard_entry(above_start + i as i64, t, false))
        .collect();

    let you = LeaderboardEntry {
        rank: my_rank,
        game_id: game.id.clone(),
        user_id: game.user_id.clone(),
        total_time_ms,
        accepted_count: game.accepted_count,
        category: game.category.clone(),
        is_you: true,
    };

    let mut below_stmt = conn.prepare(
        "SELECT id, user_id, total_time_ms, accepted_count, category
         FROM games
         WHERE category = ?1 AND target_count = ?2 AND completed_at IS NOT NULL AND total_time_ms IS NOT NULL
               AND (total_time_ms > ?3 OR (total_time_ms = ?3 AND id > ?4))
         ORDER BY total_time_ms ASC
         LIMIT ?5",
    )?;
    let below: Vec<_> = below_stmt
        .query_map(
            params![category, target_count, total_time_ms, game_id, range],
            leaderboard_row,
        )?
        .collect::<Result<Vec<_>>>()?;

    let below: Vec<_> = below
        .into_iter()
        .enumerate()
        .map(|(i, t)| to_leaderboard_entry(my_rank + i as i64 + 1, t, false))
        .collect();

    Ok(Some(Neighborhood { above, you, below }))
}

pub fn get_paginated_leaderboard(
    conn: &Connection,
    category: &str,
    target_count: i64,
    page: i64,
    per_page: i64,
) -> Result<PaginatedLeaderboard> {
    let total_entries: i64 = conn.query_row(
        "SELECT COUNT(*) FROM games WHERE category = ?1 AND target_count = ?2 AND completed_at IS NOT NULL AND total_time_ms IS NOT NULL",
        params![category, target_count],
        |row| row.get(0),
    )?;
    let total_pages = (total_entries + per_page - 1) / per_page;
    let offset = (page - 1) * per_page;

    let mut stmt = conn.prepare(
        "SELECT id, user_id, total_time_ms, accepted_count, category
         FROM games
         WHERE category = ?1 AND target_count = ?2 AND completed_at IS NOT NULL AND total_time_ms IS NOT NULL
         ORDER BY total_time_ms ASC
         LIMIT ?3 OFFSET ?4",
    )?;
    let entries: Vec<_> = stmt
        .query_map(
            params![category, target_count, per_page, offset],
            leaderboard_row,
        )?
        .collect::<Result<Vec<_>>>()?;

    let entries: Vec<_> = entries
        .into_iter()
        .enumerate()
        .map(|(i, t)| to_leaderboard_entry(offset + i as i64 + 1, t, false))
        .collect();

    Ok(PaginatedLeaderboard {
        entries,
        page,
        total_pages,
        total_entries,
    })
}

pub fn get_game_guesses(conn: &Connection, game_id: &str) -> Result<Vec<GuessSummary>> {
    let mut stmt = conn.prepare(
        "SELECT ROW_NUMBER() OVER (ORDER BY g.guess_order ASC),
                p.display_name, g.guess_time_ms, p.wikipedia_url, p.wikidata_url
         FROM guesses g
         JOIN people p ON g.person_id = p.wikidata_id
         WHERE g.game_id = ?1 AND g.accepted = 1
         ORDER BY g.guess_order ASC",
    )?;
    let rows = stmt.query_map(params![game_id], |row| {
        Ok(GuessSummary {
            order: row.get(0)?,
            display_name: row.get(1)?,
            guess_time_ms: row.get(2)?,
            wikipedia_url: row.get(3)?,
            wikidata_url: row.get(4)?,
        })
    })?;
    rows.collect()
}
