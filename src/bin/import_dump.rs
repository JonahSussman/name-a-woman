use name_a_woman::db;
use name_a_woman::models::Person;
use name_a_woman::normalize::normalize_name;

use std::io::{self, BufRead};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let gender_filter = std::env::var("IMPORT_GENDER").ok();
    let target_gender_qid = match gender_filter.as_deref() {
        Some("female") => Some("Q6581072"),
        Some("male") => Some("Q6581097"),
        Some(g) => {
            eprintln!("Invalid IMPORT_GENDER: {g}. Use 'female' or 'male'.");
            return Err("invalid gender".into());
        }
        None => None,
    };

    let max_people: u64 = std::env::var("IMPORT_MAX_PEOPLE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(u64::MAX);

    let conn = db::init_db("data/names.db")?;

    let existing: i64 = conn.query_row("SELECT COUNT(*) FROM people", [], |row| row.get(0))?;
    println!("DB before import: {existing} people");
    println!(
        "Filtering for: {} | max: {}",
        gender_filter.as_deref().unwrap_or("all genders"),
        if max_people == u64::MAX { "unlimited".to_string() } else { max_people.to_string() }
    );

    let start = std::time::Instant::now();
    let mut lines_read: u64 = 0;
    let mut imported: u64 = 0;
    let mut skipped_not_human: u64 = 0;
    let mut skipped_gender: u64 = 0;
    let mut batch_count: u64 = 0;

    let stdin = io::stdin();
    conn.execute("BEGIN", [])?;

    for line in stdin.lock().lines() {
        let line = line?;
        let line = line.trim();

        if line == "[" || line == "]" || line.is_empty() {
            continue;
        }

        let json_str = line.trim_end_matches(',');

        lines_read += 1;

        if !json_str.contains("\"Q5\"") {
            continue;
        }

        let entity: serde_json::Value = match serde_json::from_str(json_str) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let claims = match entity["claims"].as_object() {
            Some(c) => c,
            None => continue,
        };

        let is_human = claims.get("P31").and_then(|v| v.as_array()).map_or(false, |arr| {
            arr.iter().any(|claim| {
                claim["mainsnak"]["datavalue"]["value"]["id"].as_str() == Some("Q5")
            })
        });
        if !is_human {
            skipped_not_human += 1;
            continue;
        }

        let gender_id = claims
            .get("P21")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|claim| claim["mainsnak"]["datavalue"]["value"]["id"].as_str());

        if let Some(target) = target_gender_qid {
            if gender_id != Some(target) {
                skipped_gender += 1;
                continue;
            }
        }

        let gender = match gender_id {
            Some("Q6581072") => "female",
            Some("Q6581097") => "male",
            _ => "other",
        };

        let qid = match entity["id"].as_str() {
            Some(id) => id,
            None => continue,
        };

        let display_name = match entity["labels"]["en"]["value"].as_str() {
            Some(name) => name,
            None => continue,
        };

        let wikipedia_url = entity["sitelinks"]["enwiki"]["url"]
            .as_str()
            .or_else(|| {
                entity["sitelinks"]["enwiki"]["title"].as_str().map(|_| {
                    // sitelinks format varies between dump versions
                    ""
                })
            })
            .and_then(|u| if u.is_empty() { None } else { Some(u) });

        let wikidata_url = format!("http://www.wikidata.org/entity/{qid}");

        let person = Person {
            wikidata_id: qid.to_string(),
            display_name: display_name.to_string(),
            gender: gender.to_string(),
            wikipedia_url: wikipedia_url.map(|s| s.to_string()),
            wikidata_url,
        };

        db::insert_person(&conn, &person)?;

        let normalized = normalize_name(display_name);
        if !normalized.is_empty() {
            db::insert_name_variant(&conn, qid, display_name)?;
        }

        let aliases: Vec<String> = entity["aliases"]["en"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|a| a["value"].as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        for alias in &aliases {
            let normalized = normalize_name(alias);
            if !normalized.is_empty() {
                db::insert_name_variant(&conn, qid, alias)?;
            }
        }

        imported += 1;
        batch_count += 1;

        if batch_count >= 5000 {
            conn.execute("COMMIT", [])?;
            conn.execute("BEGIN", [])?;
            batch_count = 0;

            let elapsed = start.elapsed().as_secs();
            let rate = if elapsed > 0 { imported / elapsed } else { 0 };
            let remaining = if rate > 0 { max_people.saturating_sub(imported) / rate } else { 0 };
            println!(
                "[{imported}/{max_people}] {lines_read} lines scanned | {skipped_not_human} not human | {skipped_gender} wrong gender | ~{rate}/s | ~{remaining}s remaining"
            );
        }

        if imported >= max_people {
            println!("Reached import limit of {max_people}.");
            break;
        }
    }

    conn.execute("COMMIT", [])?;

    let elapsed = start.elapsed().as_secs();
    let final_count: i64 = conn.query_row("SELECT COUNT(*) FROM people", [], |row| row.get(0))?;
    let variant_count: i64 = conn.query_row("SELECT COUNT(*) FROM name_variants", [], |row| row.get(0))?;
    println!("\n=== Import complete ===");
    println!("Scanned {lines_read} entities in {elapsed}s");
    println!("Imported {imported} people (+ aliases)");
    println!("DB now has: {final_count} people, {variant_count} name variants");

    Ok(())
}
