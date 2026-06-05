use crate::models::Person;

pub async fn lookup_person(_name: &str) -> Option<Person> {
    // TODO: implement Wikidata API fallback
    None
}
