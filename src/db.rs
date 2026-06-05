use rusqlite::{Connection, Result, params};

use crate::models::*;
use crate::normalize::normalize_name;

pub fn init_db(path: &str) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL;")?;
    conn.execute_batch(include_str!("../schema.sql"))?;
    Ok(conn)
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
    conn.execute(
        "INSERT INTO name_variants (wikidata_id, name_normalized) VALUES (?1, ?2)",
        params![wikidata_id, normalized],
    )?;
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
    let rows = stmt.query_map(params![normalized], |row| {
        Ok(Person {
            wikidata_id: row.get(0)?,
            display_name: row.get(1)?,
            gender: row.get(2)?,
            wikipedia_url: row.get(3)?,
            wikidata_url: row.get(4)?,
        })
    })?;
    rows.collect()
}

pub fn lookup_fuzzy(conn: &Connection, name: &str, max_distance: usize) -> Result<Vec<(Person, String)>> {
    let normalized = normalize_name(name);
    let prefix = if normalized.len() >= 3 {
        &normalized[..3]
    } else {
        &normalized
    };

    let mut stmt = conn.prepare(
        "SELECT p.wikidata_id, p.display_name, p.gender, p.wikipedia_url, p.wikidata_url, nv.name_normalized
         FROM name_variants nv
         JOIN people p ON nv.wikidata_id = p.wikidata_id
         WHERE nv.name_normalized LIKE ?1
         LIMIT 500",
    )?;
    let like_pattern = format!("{prefix}%");
    let rows = stmt.query_map(params![like_pattern], |row| {
        Ok((
            Person {
                wikidata_id: row.get(0)?,
                display_name: row.get(1)?,
                gender: row.get(2)?,
                wikipedia_url: row.get(3)?,
                wikidata_url: row.get(4)?,
            },
            row.get::<_, String>(5)?,
        ))
    })?;

    let mut results = Vec::new();
    for row in rows {
        let (person, variant_name) = row?;
        let dist = strsim::levenshtein(&normalized, &variant_name);
        if dist <= max_distance {
            results.push((person, variant_name));
        }
    }
    Ok(results)
}

pub fn create_game(conn: &Connection, game_id: &str, user_id: &str, ip_hash: &str, category: &str, target_count: i64) -> Result<()> {
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
    pub fallback_lookups_used: i64,
    pub completed_at: Option<String>,
    pub total_time_ms: Option<i64>,
}

pub fn get_game(conn: &Connection, game_id: &str) -> Result<Option<GameRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, category, target_count, accepted_count, fallback_lookups_used, completed_at, total_time_ms
         FROM games WHERE id = ?1",
    )?;
    let mut rows = stmt.query_map(params![game_id], |row| {
        Ok(GameRow {
            id: row.get(0)?,
            user_id: row.get(1)?,
            category: row.get(2)?,
            target_count: row.get(3)?,
            accepted_count: row.get(4)?,
            fallback_lookups_used: row.get(5)?,
            completed_at: row.get(6)?,
            total_time_ms: row.get(7)?,
        })
    })?;
    match rows.next() {
        Some(row) => Ok(Some(row?)),
        None => Ok(None),
    }
}

pub fn is_person_already_guessed(conn: &Connection, game_id: &str, wikidata_id: &str) -> Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM guesses WHERE game_id = ?1 AND person_id = ?2 AND accepted = 1",
        params![game_id, wikidata_id],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

pub fn insert_guess(conn: &Connection, game_id: &str, name_entered: &str, person_id: Option<&str>, accepted: bool, guess_time_ms: i64, guess_order: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO guesses (game_id, name_entered, person_id, accepted, guess_time_ms, guess_order)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![game_id, name_entered, person_id, accepted as i64, guess_time_ms, guess_order],
    )?;
    Ok(())
}

pub fn increment_accepted_count(conn: &Connection, game_id: &str) -> Result<i64> {
    conn.execute(
        "UPDATE games SET accepted_count = accepted_count + 1 WHERE id = ?1",
        params![game_id],
    )?;
    let count: i64 = conn.query_row(
        "SELECT accepted_count FROM games WHERE id = ?1",
        params![game_id],
        |row| row.get(0),
    )?;
    Ok(count)
}

pub fn increment_fallback_lookups(conn: &Connection, game_id: &str) -> Result<i64> {
    conn.execute(
        "UPDATE games SET fallback_lookups_used = fallback_lookups_used + 1 WHERE id = ?1",
        params![game_id],
    )?;
    let count: i64 = conn.query_row(
        "SELECT fallback_lookups_used FROM games WHERE id = ?1",
        params![game_id],
        |row| row.get(0),
    )?;
    Ok(count)
}

pub fn complete_game(conn: &Connection, game_id: &str) -> Result<i64> {
    conn.execute(
        "UPDATE games SET completed_at = datetime('now'),
                          total_time_ms = CAST((julianday(datetime('now')) - julianday(started_at)) * 86400000 AS INTEGER)
         WHERE id = ?1",
        params![game_id],
    )?;
    let time_ms: i64 = conn.query_row(
        "SELECT total_time_ms FROM games WHERE id = ?1",
        params![game_id],
        |row| row.get(0),
    )?;
    Ok(time_ms)
}

pub fn get_rank(conn: &Connection, game_id: &str, category: &str, target_count: i64) -> Result<CompletionData> {
    let total_time_ms: i64 = conn.query_row(
        "SELECT total_time_ms FROM games WHERE id = ?1",
        params![game_id],
        |row| row.get(0),
    )?;

    let rank: i64 = conn.query_row(
        "SELECT COUNT(*) + 1 FROM games
         WHERE category = ?1 AND target_count = ?2 AND completed_at IS NOT NULL
               AND total_time_ms < ?3 AND id != ?4",
        params![category, target_count, total_time_ms, game_id],
        |row| row.get(0),
    )?;

    let total_players: i64 = conn.query_row(
        "SELECT COUNT(*) FROM games
         WHERE category = ?1 AND target_count = ?2 AND completed_at IS NOT NULL",
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

pub fn get_leaderboard_top(conn: &Connection, category: &str, target_count: i64, limit: i64) -> Result<Vec<LeaderboardEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, total_time_ms, accepted_count, category
         FROM games
         WHERE category = ?1 AND target_count = ?2 AND completed_at IS NOT NULL
         ORDER BY total_time_ms ASC
         LIMIT ?3",
    )?;
    let rows = stmt.query_map(params![category, target_count, limit], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?, row.get::<_, String>(3)?))
    })?;

    let mut entries = Vec::new();
    for (i, row) in rows.enumerate() {
        let (_, time, count, cat) = row?;
        entries.push(LeaderboardEntry {
            rank: (i + 1) as i64,
            total_time_ms: time,
            accepted_count: count,
            category: cat,
            is_you: false,
        });
    }
    Ok(entries)
}

pub fn get_leaderboard_bottom(conn: &Connection, category: &str, target_count: i64, limit: i64) -> Result<Vec<LeaderboardEntry>> {
    let total: i64 = conn.query_row(
        "SELECT COUNT(*) FROM games WHERE category = ?1 AND target_count = ?2 AND completed_at IS NOT NULL",
        params![category, target_count],
        |row| row.get(0),
    )?;

    let mut stmt = conn.prepare(
        "SELECT id, total_time_ms, accepted_count, category
         FROM games
         WHERE category = ?1 AND target_count = ?2 AND completed_at IS NOT NULL
         ORDER BY total_time_ms DESC
         LIMIT ?3",
    )?;
    let rows: Vec<_> = stmt.query_map(params![category, target_count, limit], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?, row.get::<_, String>(3)?))
    })?.collect::<Result<Vec<_>>>()?;

    let mut entries = Vec::new();
    for (i, (_, time, count, cat)) in rows.iter().rev().enumerate() {
        entries.push(LeaderboardEntry {
            rank: total - (rows.len() as i64) + (i as i64) + 1,
            total_time_ms: *time,
            accepted_count: *count,
            category: cat.clone(),
            is_you: false,
        });
    }
    Ok(entries)
}

pub fn get_neighborhood(conn: &Connection, game_id: &str, category: &str, target_count: i64, range: i64) -> Result<Option<Neighborhood>> {
    let game = match get_game(conn, game_id)? {
        Some(g) if g.completed_at.is_some() => g,
        _ => return Ok(None),
    };
    let total_time_ms = game.total_time_ms.unwrap_or(0);

    let rank_data = get_rank(conn, game_id, category, target_count)?;
    let my_rank = rank_data.rank;

    let mut above_stmt = conn.prepare(
        "SELECT id, total_time_ms, accepted_count, category
         FROM games
         WHERE category = ?1 AND target_count = ?2 AND completed_at IS NOT NULL
               AND (total_time_ms < ?3 OR (total_time_ms = ?3 AND id < ?4))
         ORDER BY total_time_ms DESC
         LIMIT ?5",
    )?;
    let above_rows: Vec<_> = above_stmt.query_map(params![category, target_count, total_time_ms, game_id, range], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?, row.get::<_, String>(3)?))
    })?.collect::<Result<Vec<_>>>()?;

    let mut above = Vec::new();
    for (i, (_, time, count, cat)) in above_rows.iter().rev().enumerate() {
        above.push(LeaderboardEntry {
            rank: my_rank - (above_rows.len() as i64) + (i as i64),
            total_time_ms: *time,
            accepted_count: *count,
            category: cat.clone(),
            is_you: false,
        });
    }

    let you = LeaderboardEntry {
        rank: my_rank,
        total_time_ms,
        accepted_count: game.accepted_count,
        category: game.category.clone(),
        is_you: true,
    };

    let mut below_stmt = conn.prepare(
        "SELECT id, total_time_ms, accepted_count, category
         FROM games
         WHERE category = ?1 AND target_count = ?2 AND completed_at IS NOT NULL
               AND (total_time_ms > ?3 OR (total_time_ms = ?3 AND id > ?4))
         ORDER BY total_time_ms ASC
         LIMIT ?5",
    )?;
    let below: Vec<_> = below_stmt.query_map(params![category, target_count, total_time_ms, game_id, range], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?, row.get::<_, String>(3)?))
    })?.collect::<Result<Vec<_>>>()?;

    let below = below.into_iter().enumerate().map(|(i, (_, time, count, cat))| {
        LeaderboardEntry {
            rank: my_rank + (i as i64) + 1,
            total_time_ms: time,
            accepted_count: count,
            category: cat,
            is_you: false,
        }
    }).collect();

    Ok(Some(Neighborhood { above, you, below }))
}

pub fn get_paginated_leaderboard(conn: &Connection, category: &str, target_count: i64, page: i64, per_page: i64) -> Result<PaginatedLeaderboard> {
    let total_entries: i64 = conn.query_row(
        "SELECT COUNT(*) FROM games WHERE category = ?1 AND target_count = ?2 AND completed_at IS NOT NULL",
        params![category, target_count],
        |row| row.get(0),
    )?;
    let total_pages = (total_entries + per_page - 1) / per_page;
    let offset = (page - 1) * per_page;

    let mut stmt = conn.prepare(
        "SELECT id, total_time_ms, accepted_count, category
         FROM games
         WHERE category = ?1 AND target_count = ?2 AND completed_at IS NOT NULL
         ORDER BY total_time_ms ASC
         LIMIT ?3 OFFSET ?4",
    )?;
    let entries: Vec<_> = stmt.query_map(params![category, target_count, per_page, offset], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?, row.get::<_, String>(3)?))
    })?.collect::<Result<Vec<_>>>()?;

    let entries = entries.into_iter().enumerate().map(|(i, (_, time, count, cat))| {
        LeaderboardEntry {
            rank: offset + (i as i64) + 1,
            total_time_ms: time,
            accepted_count: count,
            category: cat,
            is_you: false,
        }
    }).collect();

    Ok(PaginatedLeaderboard {
        entries,
        page,
        total_pages,
        total_entries,
    })
}

pub fn get_game_guesses(conn: &Connection, game_id: &str) -> Result<Vec<GuessSummary>> {
    let mut stmt = conn.prepare(
        "SELECT g.guess_order, p.display_name, g.guess_time_ms, p.wikipedia_url, p.wikidata_url
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
