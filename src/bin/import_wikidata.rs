use name_a_woman::db;
use name_a_woman::models::Person;
use name_a_woman::normalize::normalize_name;

use serde::Deserialize;

const SPARQL_ENDPOINT: &str = "https://query.wikidata.org/sparql";
const DEFAULT_BATCH_SIZE: u64 = 2_000;

#[derive(Debug, Deserialize)]
struct SparqlResponse {
    results: SparqlResults,
}

#[derive(Debug, Deserialize)]
struct SparqlResults {
    bindings: Vec<SparqlBinding>,
}

#[derive(Debug, Deserialize)]
struct SparqlBinding {
    person: SparqlValue,
    #[serde(rename = "personLabel")]
    person_label: SparqlValue,
    #[serde(rename = "genderLabel")]
    gender_label: SparqlValue,
    article: Option<SparqlValue>,
}

#[derive(Debug, Deserialize)]
struct SparqlValue {
    value: String,
}

#[derive(Debug, Deserialize)]
struct AliasResponse {
    results: AliasResults,
}

#[derive(Debug, Deserialize)]
struct AliasResults {
    bindings: Vec<AliasBinding>,
}

#[derive(Debug, Deserialize)]
struct AliasBinding {
    person: SparqlValue,
    alias: SparqlValue,
}

fn extract_wikidata_id(uri: &str) -> &str {
    uri.rsplit('/').next().unwrap_or(uri)
}

fn map_gender(label: &str) -> &'static str {
    let lower = label.to_lowercase();
    if lower.contains("female") {
        "female"
    } else if lower.contains("male") {
        "male"
    } else {
        "other"
    }
}

fn gender_qid(gender: &str) -> Option<&'static str> {
    match gender {
        "female" => Some("wd:Q6581072"),
        "male" => Some("wd:Q6581097"),
        _ => None,
    }
}

async fn sparql_json<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    query: &str,
) -> Result<T, Box<dyn std::error::Error>> {
    loop {
        let resp = client
            .post(SPARQL_ENDPOINT)
            .header("Accept", "application/sparql-results+json")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(format!("query={}", urlencoding::encode(query)))
            .send()
            .await;

        let resp = match resp {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Request error: {e}. Retrying in 60 seconds...");
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                continue;
            }
        };

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            eprintln!(
                "SPARQL query failed with {status}: {}",
                &body[..body.len().min(200)]
            );

            if status.as_u16() == 429 || status.as_u16() >= 500 {
                eprintln!("Retrying in 60 seconds...");
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                continue;
            }

            return Err(format!("SPARQL query failed: {status}").into());
        }

        match resp.json::<T>().await {
            Ok(data) => return Ok(data),
            Err(e) => {
                eprintln!("JSON decode error: {e}. Retrying in 30 seconds...");
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                continue;
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let max_people: u64 = std::env::var("IMPORT_MAX_PEOPLE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(u64::MAX);

    let batch_size: u64 = std::env::var("IMPORT_BATCH_SIZE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_BATCH_SIZE);

    let skip_aliases: bool = std::env::var("IMPORT_SKIP_ALIASES")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    let start_offset: u64 = std::env::var("IMPORT_START_OFFSET")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    let gender_filter = std::env::var("IMPORT_GENDER").ok();
    let gender_qid = match gender_filter.as_deref() {
        Some(g) => match gender_qid(g) {
            Some(qid) => Some(qid),
            None => {
                eprintln!("Invalid IMPORT_GENDER: {g}. Use 'female' or 'male'.");
                return Err("invalid gender".into());
            }
        },
        None => None,
    };

    let conn = db::init_db("data/names.db")?;
    let client = reqwest::Client::builder()
        .user_agent("NameAWomanBot/0.1 (https://jonahsussman.net; educational project)")
        .timeout(std::time::Duration::from_secs(120))
        .build()?;

    let gender_label = gender_filter.as_deref().unwrap_or("all");
    if max_people < u64::MAX {
        println!("=== Importing up to {max_people} {gender_label} people from Wikidata ===");
    } else {
        println!("=== Importing all {gender_label} people from Wikidata ===");
    }

    let gender_clause = match gender_qid {
        Some(qid) => format!("?person wdt:P21 {qid} .\n  "),
        None => String::new(),
    };

    let existing: i64 = conn.query_row("SELECT COUNT(*) FROM people", [], |row| row.get(0))?;
    let existing_variants: i64 =
        conn.query_row("SELECT COUNT(*) FROM name_variants", [], |row| row.get(0))?;
    println!("DB before import: {existing} people, {existing_variants} name variants");

    let import_start = std::time::Instant::now();
    let mut offset: u64 = start_offset;
    let mut total_imported: u64 = 0;
    let mut batch_num: u64 = 0;

    loop {
        batch_num += 1;
        let batch_start = std::time::Instant::now();
        println!("\n[batch {batch_num}] Fetching offset {offset}...");

        let query = format!(
            r#"SELECT ?person ?personLabel ?genderLabel ?article WHERE {{
  ?person wdt:P31 wd:Q5 .
  {gender_clause}?person wdt:P21 ?gender .
  ?person rdfs:label ?personLabel .
  FILTER(LANG(?personLabel) = "en")
  ?gender rdfs:label ?genderLabel .
  FILTER(LANG(?genderLabel) = "en")
  OPTIONAL {{
    ?article schema:about ?person ;
             schema:isPartOf <https://en.wikipedia.org/> .
  }}
}}
LIMIT {batch_size}
OFFSET {offset}"#
        );

        let data: SparqlResponse = sparql_json(&client, &query).await?;
        let count = data.results.bindings.len();

        if count == 0 {
            println!("No more results at offset {offset}. Done with people.");
            break;
        }

        conn.execute("BEGIN", [])?;
        for binding in &data.results.bindings {
            let wikidata_id = extract_wikidata_id(&binding.person.value);
            let display_name = &binding.person_label.value;
            let gender = map_gender(&binding.gender_label.value);
            let wikipedia_url = binding.article.as_ref().map(|a| a.value.as_str());
            let wikidata_url = &binding.person.value;

            let person = Person {
                wikidata_id: wikidata_id.to_string(),
                display_name: display_name.to_string(),
                gender: gender.to_string(),
                wikipedia_url: wikipedia_url.map(|s| s.to_string()),
                wikidata_url: wikidata_url.to_string(),
            };

            db::insert_person(&conn, &person)?;

            let normalized = normalize_name(display_name);
            if !normalized.is_empty() {
                db::insert_name_variant(&conn, wikidata_id, display_name)?;
            }
        }
        conn.execute("COMMIT", [])?;

        total_imported += count as u64;
        let elapsed = import_start.elapsed().as_secs();
        let rate = if elapsed > 0 {
            total_imported / elapsed
        } else {
            0
        };
        let remaining = if rate > 0 {
            (max_people.saturating_sub(total_imported)) / rate
        } else {
            0
        };
        let batch_ms = batch_start.elapsed().as_millis();
        println!(
            "[batch {batch_num}] +{count} people ({total_imported}/{max_people}) | {elapsed}s elapsed | ~{rate}/s | ~{remaining}s remaining | batch took {batch_ms}ms"
        );

        if total_imported >= max_people {
            println!("\nReached import limit of {max_people}.");
            break;
        }

        offset += batch_size;
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }

    if skip_aliases {
        println!("\n=== Skipping alias import (IMPORT_SKIP_ALIASES=1) ===");
        let people_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM people", [], |row| row.get(0))?;
        let variant_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM name_variants", [], |row| row.get(0))?;
        println!("DB stats: {people_count} people, {variant_count} name variants");
        return Ok(());
    }

    println!("\n=== Importing aliases ===");

    let alias_gender_clause = match gender_qid {
        Some(qid) => format!("?person wdt:P21 {qid} .\n  "),
        None => String::new(),
    };

    let mut alias_offset: u64 = 0;
    let mut total_aliases: u64 = 0;

    loop {
        println!("Fetching alias batch at offset {alias_offset}...");

        let query = format!(
            r#"SELECT ?person ?alias WHERE {{
  ?person wdt:P31 wd:Q5 .
  {alias_gender_clause}?person skos:altLabel ?alias .
  FILTER(LANG(?alias) = "en")
}}
LIMIT {batch_size}
OFFSET {alias_offset}"#
        );

        let data: AliasResponse = sparql_json(&client, &query).await?;
        let count = data.results.bindings.len();

        if count == 0 {
            println!("No more aliases at offset {alias_offset}. Done.");
            break;
        }

        let mut inserted: u64 = 0;
        conn.execute("BEGIN", [])?;
        for binding in &data.results.bindings {
            let wikidata_id = extract_wikidata_id(&binding.person.value);
            let exists: bool = conn
                .query_row(
                    "SELECT COUNT(*) FROM people WHERE wikidata_id = ?1",
                    [wikidata_id],
                    |row| row.get::<_, i64>(0),
                )
                .map(|c| c > 0)
                .unwrap_or(false);

            if !exists {
                continue;
            }

            let alias = &binding.alias.value;
            let normalized = normalize_name(alias);
            if !normalized.is_empty() {
                db::insert_name_variant(&conn, wikidata_id, alias)?;
                inserted += 1;
            }
        }
        conn.execute("COMMIT", [])?;

        total_aliases += inserted;
        println!("  Fetched {count} rows, inserted {inserted} aliases (total: {total_aliases})");

        alias_offset += batch_size;
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }

    println!("\n=== Import complete ===");
    println!("Total people: {total_imported}");
    println!("Total aliases: {total_aliases}");

    let people_count: i64 = conn.query_row("SELECT COUNT(*) FROM people", [], |row| row.get(0))?;
    let variant_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM name_variants", [], |row| row.get(0))?;
    println!("DB stats: {people_count} people, {variant_count} name variants");

    Ok(())
}
