use crate::models::Person;

#[async_trait::async_trait]
pub trait PersonLookup: Send + Sync {
    async fn lookup_person(&self, name: &str) -> Option<(Person, Vec<String>)>;
}

pub struct WikidataLookup;

#[async_trait::async_trait]
impl PersonLookup for WikidataLookup {
    async fn lookup_person(&self, name: &str) -> Option<(Person, Vec<String>)> {
        lookup_person(name).await
    }
}

const WIKIDATA_API: &str = "https://www.wikidata.org/w/api.php";

async fn lookup_person(name: &str) -> Option<(Person, Vec<String>)> {
    let client = reqwest::Client::builder()
        .user_agent("NameAWomanBot/0.1 (https://jonahsussman.net; educational project)")
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .ok()?;

    let search_url = format!(
        "{}?action=wbsearchentities&search={}&language=en&type=item&limit=20&format=json",
        WIKIDATA_API,
        urlencoding::encode(name),
    );
    let resp = client.get(&search_url).send().await.ok()?;
    let data: serde_json::Value = resp.json().await.ok()?;
    let results = data["search"].as_array()?;

    for result in results {
        let qid = result["id"].as_str()?;

        let entity = fetch_entity(&client, qid).await?;
        let claims = entity["claims"].as_object()?;

        let is_human = claims
            .get("P31")
            .and_then(|v| v.as_array())
            .map_or(false, |arr| {
                arr.iter().any(|claim| {
                    claim["mainsnak"]["datavalue"]["value"]["id"].as_str() == Some("Q5")
                })
            });
        if !is_human {
            continue;
        }

        let gender_id = claims
            .get("P21")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|claim| claim["mainsnak"]["datavalue"]["value"]["id"].as_str());

        let gender = match gender_id {
            Some("Q6581072") => "female",
            Some("Q6581097") => "male",
            _ => "other",
        };

        let display_name = entity["labels"]["en"]["value"].as_str().unwrap_or(name);

        let wikipedia_url = entity["sitelinks"]["enwiki"]["url"]
            .as_str()
            .map(|s| s.to_string());

        let aliases: Vec<String> = entity["aliases"]["en"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|a| a["value"].as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        return Some((
            Person {
                wikidata_id: qid.to_string(),
                display_name: display_name.to_string(),
                gender: gender.to_string(),
                wikipedia_url,
                wikidata_url: format!("http://www.wikidata.org/entity/{qid}"),
            },
            aliases,
        ));
    }

    None
}

async fn fetch_entity(client: &reqwest::Client, qid: &str) -> Option<serde_json::Value> {
    let url = format!(
        "{}?action=wbgetentities&ids={}&props=labels|aliases|claims|sitelinks/urls&languages=en&sitefilter=enwiki&format=json",
        WIKIDATA_API, qid,
    );
    let resp = client.get(&url).send().await.ok()?;
    let data: serde_json::Value = resp.json().await.ok()?;
    data["entities"][qid]
        .as_object()
        .map(|o| serde_json::Value::Object(o.clone()))
}
